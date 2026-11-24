use crate::config;
use serde::{Deserialize, Serialize};
use serenity::all::{
    ButtonStyle, ChannelId, ComponentInteraction, Context, CreateActionRow, CreateButton,
    CreateEmbed, CreateMessage, EditInteractionResponse, EditMessage, Message, MessageId, UserId,
};
use serenity::http::Http;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::AppHandle;
use tokio::time::Instant as TokioInstant;

const USER_DAILY_LIMIT: usize = 8;
const SERVER_PER_MINUTE_LIMIT: usize = 10;
const SERVER_DAILY_LIMIT: usize = 120;
const USER_COOLDOWN: Duration = Duration::from_secs(20);
const MAX_FILE_BYTES: usize = 6 * 1024 * 1024;
const MAX_FILES_BYTES: usize = 8 * 1024 * 1024;

const CH_CAPABILITIES: u64 = 1574498816315490454;
const CH_TAHSEELI: u64 = 1574498819138130083;
const CH_UNIVERSITY: u64 = 1574498822250561536;
const CH_QUESTION_LIBRARY: u64 = 1574498826809647275;
const CH_EXAM_ARENA: u64 = 1574498833386311783;

#[derive(Clone)]
pub struct StudyRuntime {
    opted_out_users: Arc<Mutex<HashSet<u64>>>,
    user_focus: Arc<Mutex<HashMap<u64, String>>>,
    user_last_request: Arc<Mutex<HashMap<u64, Instant>>>,
    server_minute: Arc<Mutex<VecDeque<Instant>>>,
    server_daily: Arc<Mutex<VecDeque<Instant>>>,
    timers: Arc<Mutex<HashMap<(u64, u64), StudyTimer>>>,
    timer_drafts: Arc<Mutex<HashMap<u64, TimerDraft>>>,
    last_quizzes: Arc<Mutex<HashMap<u64, String>>>,
    progress: Arc<Mutex<StudyProgressStore>>,
    progress_path: Arc<PathBuf>,
    timer_monitor: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct StudyProgressStore {
    members: HashMap<u64, StudentProgress>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct StudentProgress {
    points: u64,
    focus_sessions: u64,
    focus_minutes: u64,
    break_sessions: u64,
    break_minutes: u64,
    day_key: u64,
    daily_points: u64,
    ai_day_key: u64,
    ai_requests_today: u32,
    opted_out: bool,
    privacy_notice_sent: bool,
}

#[derive(Clone, Copy)]
struct TimerDraft {
    focus_minutes: u64,
    break_minutes: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum TimerPhase {
    Focus,
    Break,
    Completed,
    Stopped,
}

#[derive(Clone)]
struct StudyTimer {
    channel_id: ChannelId,
    message_id: MessageId,
    owner_id: UserId,
    focus_minutes: u64,
    break_minutes: u64,
    sessions_target: u64,
    sessions_completed: u64,
    phase: TimerPhase,
    deadline: Option<TokioInstant>,
    remaining: Duration,
    paused: bool,
}

fn day_key() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() / 86_400)
        .unwrap_or(0)
}

impl StudyRuntime {
    pub fn new(app: &AppHandle) -> Self {
        let path = config::load_snapshot(app)
            .map(|snapshot| PathBuf::from(snapshot.data_dir).join("study-progress.json"))
            .unwrap_or_else(|_| std::env::temp_dir().join("DAS-study-progress.json"));
        let progress = std::fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str::<StudyProgressStore>(&content).ok())
            .unwrap_or_default();
        Self {
            opted_out_users: Arc::new(Mutex::new(HashSet::new())),
            user_focus: Arc::new(Mutex::new(HashMap::new())),
            user_last_request: Arc::new(Mutex::new(HashMap::new())),
            server_minute: Arc::new(Mutex::new(VecDeque::new())),
            server_daily: Arc::new(Mutex::new(VecDeque::new())),
            timers: Arc::new(Mutex::new(HashMap::new())),
            timer_drafts: Arc::new(Mutex::new(HashMap::new())),
            last_quizzes: Arc::new(Mutex::new(HashMap::new())),
            progress: Arc::new(Mutex::new(progress)),
            progress_path: Arc::new(path),
            timer_monitor: Arc::new(Mutex::new(None)),
        }
    }

    fn persist_progress(&self) {
        let snapshot = match self.progress.lock() {
            Ok(progress) => progress.clone(),
            Err(_) => return,
        };
        let Ok(serialized) = serde_json::to_vec_pretty(&snapshot) else {
            return;
        };
        if let Some(parent) = self.progress_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temporary = self.progress_path.with_extension("json.tmp");
        if std::fs::write(&temporary, serialized).is_ok() {
            if self.progress_path.exists() {
                let _ = std::fs::remove_file(self.progress_path.as_ref());
            }
            let _ = std::fs::rename(&temporary, self.progress_path.as_ref());
        }
    }

    pub fn start_timer_monitor(&self, http: Arc<Http>) {
        let Ok(mut task) = self.timer_monitor.lock() else {
            return;
        };
        if task.as_ref().is_some_and(|current| !current.is_finished()) {
            return;
        }
        let timers = self.timers.clone();
        let runtime = self.clone();
        *task = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let snapshots = timers
                    .lock()
                    .map(|items| {
                        items
                            .iter()
                            .map(|(id, timer)| (*id, timer.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                for (id, mut timer) in snapshots {
                    if matches!(timer.phase, TimerPhase::Completed | TimerPhase::Stopped) {
                        continue;
                    }
                    let remaining = if timer.paused {
                        timer.remaining
                    } else {
                        timer
                            .deadline
                            .map(|deadline| deadline.saturating_duration_since(TokioInstant::now()))
                            .unwrap_or_default()
                    };

                    if remaining.is_zero() && !timer.paused {
                        let now = TokioInstant::now();
                        match timer.phase {
                            TimerPhase::Focus => {
                                timer.sessions_completed += 1;
                                runtime
                                    .record_focus_completion(timer.owner_id, timer.focus_minutes);
                                if timer.sessions_completed >= timer.sessions_target {
                                    timer.phase = TimerPhase::Completed;
                                    timer.deadline = None;
                                    timer.remaining = Duration::ZERO;
                                } else {
                                    timer.phase = TimerPhase::Break;
                                    timer.deadline =
                                        Some(now + Duration::from_secs(timer.break_minutes * 60));
                                    timer.remaining = Duration::from_secs(timer.break_minutes * 60);
                                }
                            }
                            TimerPhase::Break => {
                                runtime
                                    .record_break_completion(timer.owner_id, timer.break_minutes);
                                timer.phase = TimerPhase::Focus;
                                timer.deadline =
                                    Some(now + Duration::from_secs(timer.focus_minutes * 60));
                                timer.remaining = Duration::from_secs(timer.focus_minutes * 60);
                            }
                            TimerPhase::Completed | TimerPhase::Stopped => {}
                        }
                        if let Ok(mut items) = timers.lock() {
                            if let Some(stored) = items.get_mut(&id) {
                                *stored = timer.clone();
                            }
                        }
                    }

                    let edit = EditMessage::new()
                        .content(timer_message(&timer))
                        .components(timer_components(&timer));
                    let _ = timer
                        .channel_id
                        .edit_message(http.as_ref(), timer.message_id, edit)
                        .await;
                }
            }
        }));
    }

    fn is_opted_out(&self, user: UserId) -> bool {
        let stored = self
            .progress
            .lock()
            .map(|items| {
                items
                    .members
                    .get(&user.get())
                    .map(|entry| entry.opted_out)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        stored
            || self
                .opted_out_users
                .lock()
                .map(|items| items.contains(&user.get()))
                .unwrap_or(false)
    }

    fn set_opted_out(&self, user: UserId, opted_out: bool) {
        if let Ok(mut items) = self.opted_out_users.lock() {
            if opted_out {
                items.insert(user.get());
            } else {
                items.remove(&user.get());
            }
        }
        if let Ok(mut items) = self.progress.lock() {
            items.members.entry(user.get()).or_default().opted_out = opted_out;
        }
        self.persist_progress();
    }

    fn mark_privacy_notice(&self, user: UserId) -> bool {
        let mut should_send = false;
        if let Ok(mut items) = self.progress.lock() {
            let entry = items.members.entry(user.get()).or_default();
            if !entry.privacy_notice_sent {
                entry.privacy_notice_sent = true;
                should_send = true;
            }
        }
        if should_send {
            self.persist_progress();
        }
        should_send
    }

    fn set_focus(&self, user: UserId, focus: impl Into<String>) {
        if let Ok(mut items) = self.user_focus.lock() {
            items.insert(user.get(), focus.into());
        }
    }

    fn focus(&self, user: UserId, channel: ChannelId) -> String {
        if let Ok(items) = self.user_focus.lock() {
            if let Some(focus) = items.get(&user.get()) {
                return focus.clone();
            }
        }
        match channel.get() {
            CH_CAPABILITIES => "القدرات — حدّد الكمي أو اللفظي".to_owned(),
            CH_TAHSEELI => "التحصيلي — حدّد المادة: رياضيات، فيزياء، كيمياء، أحياء".to_owned(),
            CH_UNIVERSITY => "الجامعة — اذكر اسم المقرر والموضوع".to_owned(),
            CH_QUESTION_LIBRARY => "مكتبة الأسئلة — حدّد المسار والمادة".to_owned(),
            CH_EXAM_ARENA => "ساحة الاختبارات — حدّد المادة والموضوع".to_owned(),
            _ => "التعلّم الأكاديمي".to_owned(),
        }
    }

    fn remember_quiz(&self, user: UserId, quiz: String) {
        if let Ok(mut items) = self.last_quizzes.lock() {
            items.insert(user.get(), quiz);
        }
    }

    fn has_quiz(&self, user: UserId) -> bool {
        self.last_quizzes
            .lock()
            .map(|items| items.contains_key(&user.get()))
            .unwrap_or(false)
    }

    fn take_quiz(&self, user: UserId) -> Option<String> {
        self.last_quizzes
            .lock()
            .ok()
            .and_then(|mut items| items.remove(&user.get()))
    }

    fn can_request(&self, user: UserId) -> Result<(), String> {
        let now = Instant::now();
        if let Ok(mut last) = self.user_last_request.lock() {
            if let Some(previous) = last.get(&user.get()) {
                if now.duration_since(*previous) < USER_COOLDOWN {
                    let wait = USER_COOLDOWN
                        .saturating_sub(now.duration_since(*previous))
                        .as_secs()
                        .max(1);
                    return Err(format!(
                        "خذ استراحة قصيرة؛ يمكنك طلب تحليل جديد بعد {wait} ثانية."
                    ));
                }
            }
            last.insert(user.get(), now);
        }

        if let Ok(mut minute) = self.server_minute.lock() {
            while minute
                .front()
                .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(60))
            {
                minute.pop_front();
            }
            if minute.len() >= SERVER_PER_MINUTE_LIMIT {
                return Err("الطلب على المساعد مرتفع الآن. انتظر دقيقة ثم أعد المحاولة.".to_owned());
            }
        }
        if let Ok(mut daily) = self.server_daily.lock() {
            while daily
                .front()
                .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(86_400))
            {
                daily.pop_front();
            }
            if daily.len() >= SERVER_DAILY_LIMIT {
                return Err(
                    "وصل السيرفر إلى الحد اليومي التجريبي للتحليل. تتجدد الحصة خلال 24 ساعة."
                        .to_owned(),
                );
            }
        }

        let today = day_key();
        {
            let mut store = self
                .progress
                .lock()
                .map_err(|_| "تعذر قراءة سجل الاستخدام.".to_owned())?;
            let member = store.members.entry(user.get()).or_default();
            if member.ai_day_key != today {
                member.ai_day_key = today;
                member.ai_requests_today = 0;
            }
            if member.ai_requests_today >= USER_DAILY_LIMIT as u32 {
                return Err(format!(
                    "وصلت إلى حدك اليومي: {USER_DAILY_LIMIT} تحليلات. يتجدد الحد في اليوم التالي."
                ));
            }
            member.ai_requests_today += 1;
        }
        self.persist_progress();
        if let Ok(mut minute) = self.server_minute.lock() {
            minute.push_back(now);
        }
        if let Ok(mut daily) = self.server_daily.lock() {
            daily.push_back(now);
        }
        Ok(())
    }

    fn record_focus_completion(&self, user: UserId, minutes: u64) {
        if let Ok(mut store) = self.progress.lock() {
            let member = store.members.entry(user.get()).or_default();
            let today = day_key();
            if member.day_key != today {
                member.day_key = today;
                member.daily_points = 0;
            }
            member.focus_sessions += 1;
            member.focus_minutes += minutes;
            let reward = 10_u64.min(50_u64.saturating_sub(member.daily_points));
            member.points += reward;
            member.daily_points += reward;
        }
        self.persist_progress();
    }

    fn record_break_completion(&self, user: UserId, minutes: u64) {
        if let Ok(mut store) = self.progress.lock() {
            let member = store.members.entry(user.get()).or_default();
            member.break_sessions += 1;
            member.break_minutes += minutes;
        }
        self.persist_progress();
    }

    fn set_timer_focus(&self, user: UserId, minutes: u64) {
        if let Ok(mut drafts) = self.timer_drafts.lock() {
            drafts.insert(
                user.get(),
                TimerDraft {
                    focus_minutes: minutes,
                    break_minutes: 5,
                },
            );
        }
    }

    fn set_timer_break(&self, user: UserId, minutes: u64) -> Result<(), String> {
        let mut drafts = self
            .timer_drafts
            .lock()
            .map_err(|_| "تعذر حفظ إعدادات المؤقت.".to_owned())?;
        let draft = drafts
            .get_mut(&user.get())
            .ok_or_else(|| "ابدأ باختيار مدة التركيز أولًا.".to_owned())?;
        draft.break_minutes = minutes;
        Ok(())
    }

    fn take_timer_draft(&self, user: UserId) -> Option<TimerDraft> {
        self.timer_drafts
            .lock()
            .ok()
            .and_then(|mut drafts| drafts.remove(&user.get()))
    }

    fn progress_embed(&self, user: UserId) -> CreateEmbed {
        let snapshot = self
            .progress
            .lock()
            .map(|store| store.clone())
            .unwrap_or_default();
        let member = snapshot
            .members
            .get(&user.get())
            .cloned()
            .unwrap_or_default();
        let achievements = [
            (member.focus_sessions >= 1, "🌱 أول جلسة"),
            (member.focus_sessions >= 5, "🔥 خمس جلسات"),
            (member.focus_sessions >= 10, "💪 عشر جلسات"),
            (member.focus_sessions >= 25, "🏅 25 جلسة"),
            (member.focus_sessions >= 50, "⭐ 50 جلسة"),
        ]
        .iter()
        .filter(|(earned, _)| *earned)
        .map(|(_, label)| *label)
        .collect::<Vec<_>>();
        let achievement_text = if achievements.is_empty() {
            "ابدأ أول جلسة لفتح إنجازك الأول.".to_owned()
        } else {
            achievements.join(" • ")
        };
        CreateEmbed::new()
            .title("🏆 لوحة إنجازك — DAS")
            .description(format!(
                "العضو: <@{}>\n\n**إنجازاتك:** {achievement_text}",
                user.get()
            ))
            .field("نقاط الإنجاز", format!("{} نقطة", member.points), true)
            .field("نقاط اليوم", format!("{} / 50", member.daily_points), true)
            .field("جلسات التركيز", format!("{}", member.focus_sessions), true)
            .field(
                "وقت التركيز",
                format!("{} دقيقة", member.focus_minutes),
                true,
            )
            .field("جلسات البريك", format!("{}", member.break_sessions), true)
            .field(
                "وقت البريك",
                format!("{} دقيقة", member.break_minutes),
                true,
            )
            .field(
                "التحليلات اليوم",
                format!("{}/{}", member.ai_requests_today, USER_DAILY_LIMIT),
                true,
            )
            .colour(0xF2C500)
    }

    pub fn leaderboard_embed(&self) -> CreateEmbed {
        let snapshot = self
            .progress
            .lock()
            .map(|store| store.clone())
            .unwrap_or_default();
        let mut members = snapshot.members.into_iter().collect::<Vec<_>>();
        members.retain(|(_, member)| member.focus_sessions > 0);
        members.sort_by(|left, right| right.1.points.cmp(&left.1.points));
        let rows = members
            .iter()
            .take(10)
            .enumerate()
            .map(|(index, (id, member))| {
                format!(
                    "**{}.** <@{}> — {} نقطة · {} جلسة",
                    index + 1,
                    id,
                    member.points,
                    member.focus_sessions
                )
            })
            .collect::<Vec<_>>();
        let description = if rows.is_empty() {
            "لا توجد إنجازات مسجلة بعد. ابدأ أول جلسة تركيز!".to_owned()
        } else {
            rows.join("\n")
        };
        CreateEmbed::new()
            .title("🏆 لوحة إنجاز DAS")
            .description(description)
            .footer(serenity::all::CreateEmbedFooter::new(
                "تُحتسب النقاط تلقائيًا عند اكتمال جلسة التركيز",
            ))
            .colour(0xF2C500)
    }

    async fn start_timer(
        &self,
        context: &Context,
        component: &ComponentInteraction,
        focus_minutes: u64,
        break_minutes: u64,
        sessions_target: u64,
    ) -> Result<(), String> {
        if !matches!(focus_minutes, 25 | 40 | 50)
            || !matches!(break_minutes, 5 | 10 | 15)
            || !(1..=4).contains(&sessions_target)
        {
            return Err(
                "إعدادات الجلسة غير صالحة. اختر تركيز 25/40/50، وبريك 5/10/15، وعدد 1 إلى 4 جلسات."
                    .to_owned(),
            );
        }
        let channel = component.channel_id;
        let timer_key = (channel.get(), component.user.id.get());
        if let Ok(items) = self.timers.lock() {
            if let Some(existing) = items.get(&timer_key) {
                if !matches!(existing.phase, TimerPhase::Completed | TimerPhase::Stopped)
                    && existing.owner_id != component.user.id
                {
                    return Err("هناك جلسة مؤقت نشطة بدأها عضو آخر في هذا الروم. انتظر نهايتها أو استخدم روم تركيز منفصل.".to_owned());
                }
                if !matches!(existing.phase, TimerPhase::Completed | TimerPhase::Stopped) {
                    return Err(
                        "لديك جلسة نشطة بالفعل في هذا الروم. أكملها أو أنهها قبل بدء أخرى."
                            .to_owned(),
                    );
                }
            }
        }
        let duration = Duration::from_secs(focus_minutes * 60);
        let mut timer = StudyTimer {
            channel_id: channel,
            message_id: MessageId::new(1),
            owner_id: component.user.id,
            focus_minutes,
            break_minutes,
            sessions_target,
            sessions_completed: 0,
            phase: TimerPhase::Focus,
            deadline: Some(TokioInstant::now() + duration),
            remaining: duration,
            paused: false,
        };
        let message = channel
            .send_message(
                &context.http,
                CreateMessage::new()
                    .content(timer_message(&timer))
                    .components(timer_components(&timer)),
            )
            .await
            .map_err(|error| format!("تعذر إنشاء رسالة المؤقت: {error}"))?;
        timer.message_id = message.id;
        if let Ok(mut items) = self.timers.lock() {
            items.insert(timer_key, timer);
        }
        Ok(())
    }

    async fn control_timer(
        &self,
        context: &Context,
        component: &ComponentInteraction,
        action: &str,
    ) -> Result<String, String> {
        let key = (component.channel_id.get(), component.user.id.get());
        let mut timer = {
            let items = self
                .timers
                .lock()
                .map_err(|_| "تعذر قراءة حالة المؤقت.".to_owned())?;
            items
                .get(&key)
                .cloned()
                .ok_or_else(|| "لا يوجد مؤقت نشط في هذا الروم.".to_owned())?
        };
        if timer.message_id != component.message.id {
            return Err("هذه رسالة مؤقت قديمة. استخدم أحدث مؤقت في الروم.".to_owned());
        }
        if timer.owner_id != component.user.id {
            return Err("التحكم بالمؤقت متاح لصاحب الجلسة فقط.".to_owned());
        }
        if matches!(timer.phase, TimerPhase::Completed | TimerPhase::Stopped) {
            return Err("انتهت هذه الجلسة بالفعل.".to_owned());
        }
        match action {
            "pause" if !timer.paused => {
                timer.remaining = timer
                    .deadline
                    .map(|end| end.saturating_duration_since(TokioInstant::now()))
                    .unwrap_or_default();
                timer.deadline = None;
                timer.paused = true;
            }
            "resume" if timer.paused && !timer.remaining.is_zero() => {
                timer.deadline = Some(TokioInstant::now() + timer.remaining);
                timer.paused = false;
            }
            "stop" => {
                timer.phase = TimerPhase::Stopped;
                timer.paused = false;
                timer.deadline = None;
                timer.remaining = Duration::ZERO;
            }
            _ => return Err("تعذر تنفيذ هذا الإجراء على حالة المؤقت الحالية.".to_owned()),
        }
        if let Ok(mut items) = self.timers.lock() {
            items.insert(key, timer.clone());
        }
        timer
            .channel_id
            .edit_message(
                &context.http,
                timer.message_id,
                EditMessage::new()
                    .content(timer_message(&timer))
                    .components(timer_components(&timer)),
            )
            .await
            .map_err(|error| format!("تعذر تحديث المؤقت: {error}"))?;
        Ok(match action {
            "pause" => "تم إيقاف المؤقت مؤقتًا.".to_owned(),
            "resume" => "استؤنفت جلسة المذاكرة.".to_owned(),
            "stop" => "تم إنهاء دورة المذاكرة. لا تُحتسب جلسة التركيز غير المكتملة.".to_owned(),
            _ => "تم تحديث المؤقت.".to_owned(),
        })
    }
}

fn timer_remaining(timer: &StudyTimer) -> u64 {
    if timer.paused {
        timer.remaining.as_secs()
    } else if matches!(timer.phase, TimerPhase::Completed | TimerPhase::Stopped) {
        0
    } else {
        timer
            .deadline
            .map(|end| end.saturating_duration_since(TokioInstant::now()).as_secs())
            .unwrap_or_default()
    }
}

fn timer_message(timer: &StudyTimer) -> String {
    let phase = match timer.phase {
        TimerPhase::Focus => "🎯 وقت التركيز",
        TimerPhase::Break => "☕ وقت البريك",
        TimerPhase::Completed => "🏁 اكتملت الخطة",
        TimerPhase::Stopped => "⏹️ أُوقفت الجلسة",
    };
    let next = if timer.phase == TimerPhase::Focus {
        format!("بعدها بريك {} دقائق", timer.break_minutes)
    } else if timer.phase == TimerPhase::Break {
        format!("بعدها جلسة تركيز {} دقيقة", timer.focus_minutes)
    } else {
        "راجع إنجازك في لوحة الإنجازات.".to_owned()
    };
    format!(
        "⏳ **جلسة مذاكرة DAS**\n**{phase} — الجلسة {}/{}**\n**الوقت المتبقي: {}**\n{}\nصاحب الجلسة: <@{}>",
        timer.sessions_completed + if timer.phase == TimerPhase::Focus { 1 } else { 0 },
        timer.sessions_target,
        clock(timer_remaining(timer)),
        next,
        timer.owner_id.get()
    )
}

fn timer_components(timer: &StudyTimer) -> Vec<CreateActionRow> {
    let buttons = if matches!(timer.phase, TimerPhase::Completed | TimerPhase::Stopped) {
        vec![CreateButton::new("das:timer:stop")
            .label("انتهت الجلسة")
            .style(ButtonStyle::Secondary)
            .disabled(true)]
    } else if timer.paused {
        vec![
            CreateButton::new("das:timer:resume")
                .label("استئناف")
                .style(ButtonStyle::Success),
            CreateButton::new("das:timer:stop")
                .label("إنهاء")
                .style(ButtonStyle::Danger),
        ]
    } else {
        vec![
            CreateButton::new("das:timer:pause")
                .label("إيقاف مؤقت")
                .style(ButtonStyle::Secondary),
            CreateButton::new("das:timer:stop")
                .label("إنهاء")
                .style(ButtonStyle::Danger),
        ]
    };
    vec![CreateActionRow::Buttons(buttons)]
}
fn clock(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

pub fn menu_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("🎓 رحلة الطالب — DAS Bot")
        .description(
            "اختر ما تحتاجه الآن:\n\n\
             **🧮 القدرات:** الكمي واللفظي.\n\
             **🧪 التحصيلي:** الرياضيات والفيزياء والكيمياء والأحياء.\n\
             **🎒 الجامعة:** شرح المحاضرات والمفاهيم وحل المسائل.\n\
             **🎯 ساحة الاختبارات:** تدريب حسب المادة.\n\
             **⏳ مؤقت المذاكرة:** تركيز + بريك + عدد جلسات.\n\n\
             **المساعدة الآلية:** عند إرسال سؤال أو ملف داخل قنوات الدراسة، يحلله DAS Bot عبر Google Gemini تلقائيًا. لا ترسل بيانات شخصية أو ملفات خاصة. قد تستخدم Google محتوى المستوى المجاني لتحسين خدماتها. يمكنك إيقاف التحليل لنفسك من الأزرار."
        )
        .colour(0xF2C500)
}

pub fn menu_components() -> Vec<CreateActionRow> {
    vec![
        CreateActionRow::Buttons(vec![
            CreateButton::new("das:study:abilities")
                .label("🧮 القدرات")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:study:tahseeli")
                .label("🧪 التحصيلي")
                .style(ButtonStyle::Success),
            CreateButton::new("das:study:university")
                .label("🎒 الجامعة")
                .style(ButtonStyle::Secondary),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new("das:study:exam")
                .label("🎯 ساحة الاختبارات")
                .style(ButtonStyle::Success),
            CreateButton::new("das:study:timer")
                .label("⏳ مؤقت المذاكرة")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:study:board")
                .label("📊 إنجازي")
                .style(ButtonStyle::Success),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new("das:study:leaderboard")
                .label("🏆 لوحة الصدارة")
                .style(ButtonStyle::Success),
            CreateButton::new("das:assistant:off")
                .label("إيقاف المساعدة الآلية")
                .style(ButtonStyle::Danger),
            CreateButton::new("das:assistant:on")
                .label("تفعيل المساعدة الآلية")
                .style(ButtonStyle::Success),
        ]),
    ]
}
fn subject_components(kind: &str) -> Vec<CreateActionRow> {
    match kind {
        "abilities" => vec![CreateActionRow::Buttons(vec![
            CreateButton::new("das:focus:القدرات الكمي")
                .label("🧮 الكمي")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:focus:القدرات اللفظي")
                .label("📚 اللفظي")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary),
        ])],
        "tahseeli" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:focus:التحصيلي الرياضيات")
                    .label("➗ الرياضيات")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:focus:التحصيلي الفيزياء")
                    .label("⚛️ الفيزياء")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:focus:التحصيلي الكيمياء")
                    .label("🧪 الكيمياء")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:focus:التحصيلي الأحياء")
                    .label("🧬 الأحياء")
                    .style(ButtonStyle::Success),
            ]),
            CreateActionRow::Buttons(vec![CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary)]),
        ],
        "exam" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:exam:abilities")
                    .label("القدرات")
                    .style(ButtonStyle::Primary),
                CreateButton::new("das:exam:tahseeli")
                    .label("التحصيلي")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:exam:university")
                    .label("الجامعة")
                    .style(ButtonStyle::Secondary),
            ]),
            CreateActionRow::Buttons(vec![CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary)]),
        ],
        "exam-abilities" => vec![CreateActionRow::Buttons(vec![
            CreateButton::new("das:examfocus:القدرات الكمي")
                .label("كمي")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:examfocus:القدرات اللفظي")
                .label("لفظي")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary),
        ])],
        "exam-tahseeli" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:examfocus:التحصيلي الرياضيات")
                    .label("➗ رياضيات")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:examfocus:التحصيلي الفيزياء")
                    .label("⚛️ فيزياء")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:examfocus:التحصيلي الكيمياء")
                    .label("🧪 كيمياء")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:examfocus:التحصيلي الأحياء")
                    .label("🧬 أحياء")
                    .style(ButtonStyle::Success),
            ]),
            CreateActionRow::Buttons(vec![CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary)]),
        ],
        "quiz-count" => vec![CreateActionRow::Buttons(vec![
            CreateButton::new("das:quiz:5")
                .label("📝 اختبار 5 أسئلة")
                .style(ButtonStyle::Success),
            CreateButton::new("das:quiz:10")
                .label("📋 اختبار 10 أسئلة")
                .style(ButtonStyle::Primary),
            CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary),
        ])],
        "timer" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:timer:start:25")
                    .label("🎯 تركيز 25 د")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:timer:start:40")
                    .label("⏱️ تركيز 40 د")
                    .style(ButtonStyle::Primary),
                CreateButton::new("das:timer:start:50")
                    .label("🔥 تركيز 50 د")
                    .style(ButtonStyle::Primary),
            ]),
            CreateActionRow::Buttons(vec![CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary)]),
        ],
        "timer-break" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:timer:break:5")
                    .label("☕ بريك 5 د")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:timer:break:10")
                    .label("🌿 بريك 10 د")
                    .style(ButtonStyle::Primary),
                CreateButton::new("das:timer:break:15")
                    .label("🧘 بريك 15 د")
                    .style(ButtonStyle::Primary),
            ]),
            CreateActionRow::Buttons(vec![CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary)]),
        ],
        "timer-sessions" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:timer:sessions:1")
                    .label("1️⃣ جلسة واحدة")
                    .style(ButtonStyle::Secondary),
                CreateButton::new("das:timer:sessions:2")
                    .label("2️⃣ جلستان")
                    .style(ButtonStyle::Primary),
                CreateButton::new("das:timer:sessions:3")
                    .label("3️⃣ 3 جلسات")
                    .style(ButtonStyle::Primary),
                CreateButton::new("das:timer:sessions:4")
                    .label("4️⃣ 4 جلسات")
                    .style(ButtonStyle::Success),
            ]),
            CreateActionRow::Buttons(vec![CreateButton::new("das:study:back")
                .label("رجوع")
                .style(ButtonStyle::Secondary)]),
        ],
        "university" => vec![
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:uni:explain")
                    .label("💡 شرح مفهوم")
                    .style(ButtonStyle::Primary),
                CreateButton::new("das:uni:solve")
                    .label("✍️ حل مسألة")
                    .style(ButtonStyle::Secondary),
                CreateButton::new("das:uni:summary")
                    .label("📖 تلخيص محاضرة")
                    .style(ButtonStyle::Primary),
            ]),
            CreateActionRow::Buttons(vec![
                CreateButton::new("das:uni:quiz")
                    .label("🎯 اختبرني")
                    .style(ButtonStyle::Success),
                CreateButton::new("das:uni:plan")
                    .label("🗓️ خطة مذاكرة")
                    .style(ButtonStyle::Secondary),
                CreateButton::new("das:study:back")
                    .label("رجوع")
                    .style(ButtonStyle::Secondary),
            ]),
        ],
        _ => vec![CreateActionRow::Buttons(vec![CreateButton::new(
            "das:study:back",
        )
        .label("رجوع للقائمة")
        .style(ButtonStyle::Secondary)])],
    }
}

fn room_colour(custom_id: &str) -> u32 {
    let id = custom_id.to_lowercase();
    if id.contains("القدرات") || id.contains("abilities") {
        0x3B82F6
    } else if id.contains("التحصيلي") || id.contains("tahseeli") {
        0x22C55E
    } else if id.contains("الجامعة") || id.contains("university") || id.contains("das:uni:")
    {
        0x8B5CF6
    } else if id.contains("exam") || id.contains("quiz") || id.contains("اختبار") {
        0xF59E0B
    } else if id.contains("timer") || id.contains("مؤقت") || id.contains("جلسة") {
        0x06B6D4
    } else if id.contains("board") || id.contains("leaderboard") || id.contains("إنجاز") {
        0xEAB308
    } else {
        0xF2C500
    }
}

async fn edit_panel(
    component: &ComponentInteraction,
    context: &Context,
    title: &str,
    description: &str,
    rows: Vec<CreateActionRow>,
) {
    let embed = CreateEmbed::new()
        .title(title)
        .description(description)
        .colour(room_colour(&component.data.custom_id));
    let _ = component
        .edit_response(
            &context.http,
            EditInteractionResponse::new().embed(embed).components(rows),
        )
        .await;
}

pub async fn handle_component(
    app: &AppHandle,
    runtime: &StudyRuntime,
    context: &Context,
    component: &ComponentInteraction,
) -> bool {
    let id = component.data.custom_id.as_str();
    if !id.starts_with("das:") {
        return false;
    }

    if id == "das:timer:pause" || id == "das:timer:resume" || id == "das:timer:stop" {
        let _ = component.defer_ephemeral(&context.http).await;
        let action = id.rsplit(':').next().unwrap_or_default();
        let text = runtime
            .control_timer(context, component, action)
            .await
            .unwrap_or_else(|error| error);
        let _ = component
            .edit_response(&context.http, EditInteractionResponse::new().content(text))
            .await;
        return true;
    }

    let _ = component.defer_ephemeral(&context.http).await;

    if id == "das:assistant:off" {
        runtime.set_opted_out(component.user.id, true);
        edit_panel(component, context, "تم إيقاف المساعدة الآلية", "لن يرسل DAS Bot رسائلك أو ملفاتك إلى Gemini. يمكنك إعادة تفعيلها من زر «تفعيل المساعدة الآلية» في /study.", menu_components()).await;
        return true;
    }
    if id == "das:assistant:on" {
        runtime.set_opted_out(component.user.id, false);
        edit_panel(component, context, "تم تفعيل المساعدة الآلية", "سيحلل DAS Bot أسئلتك وملفاتك داخل قنوات الدراسة تلقائيًا. الحد 8 تحليلات يوميًا، مع فاصل 20 ثانية.", menu_components()).await;
        return true;
    }
    if id == "das:study:board" {
        let _ = component
            .edit_response(
                &context.http,
                EditInteractionResponse::new()
                    .embed(runtime.progress_embed(component.user.id))
                    .components(menu_components()),
            )
            .await;
        return true;
    }
    if id == "das:study:leaderboard" {
        let _ = component
            .edit_response(
                &context.http,
                EditInteractionResponse::new()
                    .embed(runtime.leaderboard_embed())
                    .components(menu_components()),
            )
            .await;
        return true;
    }
    if id == "das:study:back" {
        edit_panel(
            component,
            context,
            "🎓 رحلة الطالب — DAS Bot",
            "اختر المسار أو الأداة التي تحتاجها.",
            menu_components(),
        )
        .await;
        return true;
    }

    match id {
        "das:study:abilities" => {
            edit_panel(
                component,
                context,
                "🧮 مختبر القدرات",
                "اختر القسم الذي تذاكر له.",
                subject_components("abilities"),
            )
            .await;
        }
        "das:study:tahseeli" => {
            edit_panel(
                component,
                context,
                "🧪 مختبر التحصيلي",
                "اختر المادة التي تريد مراجعتها.",
                subject_components("tahseeli"),
            )
            .await;
        }
        "das:study:university" => {
            runtime.set_focus(
                component.user.id,
                "الدراسة الجامعية — اذكر اسم المقرر والموضوع أو رقم الصفحة.",
            );
            edit_panel(
                component,
                context,
                "🎒 رحلة الجامعة",
                "اختر نوع المساعدة، ثم اكتب اسم المقرر والموضوع أو أرفق المحاضرة في روم الجامعة.",
                subject_components("university"),
            )
            .await;
        }
        "das:study:exam" => {
            edit_panel(
                component,
                context,
                "🎯 ساحة الاختبارات",
                "اختر المسار، ثم المادة، ثم عدد الأسئلة.",
                subject_components("exam"),
            )
            .await;
        }
        "das:study:timer" => {
            edit_panel(component, context, "⏳ مؤقت المذاكرة", "1) اختر مدة التركيز. 2) اختر مدة البريك. 3) حدد عدد جلسات التركيز. يبدأ بعدها المؤقت بالتناوب تلقائيًا.", subject_components("timer")).await;
        }
        "das:exam:abilities" => {
            edit_panel(
                component,
                context,
                "اختبار القدرات",
                "اختر القسم.",
                subject_components("exam-abilities"),
            )
            .await;
        }
        "das:exam:tahseeli" => {
            edit_panel(
                component,
                context,
                "اختبار التحصيلي",
                "اختر المادة.",
                subject_components("exam-tahseeli"),
            )
            .await;
        }
        "das:exam:university" => {
            runtime.set_focus(component.user.id, "اختبار جامعي — اذكر اسم المقرر والوحدة.");
            edit_panel(
                component,
                context,
                "اختبار جامعي",
                "اكتب اسم المقرر والوحدة في الشات، ثم اختر عدد الأسئلة.",
                subject_components("quiz-count"),
            )
            .await;
        }
        _ if id.starts_with("das:uni:") => {
            let mode = id.trim_start_matches("das:uni:");
            let focus = match mode {
                "explain" => "الجامعة — اشرح المفهوم تدريجيًا بالتعريف ثم المثال والربط.",
                "solve" => "الجامعة — حل المسألة خطوة بخطوة واشرح القوانين والوحدات.",
                "summary" => "الجامعة — لخّص المحاضرة أو الملف مع المفاهيم والتعاريف والقوانين.",
                "quiz" => "الجامعة — أنشئ مراجعة واختبارًا تدريبيًا للمقرر.",
                "plan" => "الجامعة — ساعد الطالب على بناء خطة مذاكرة واقعية للمقرر.",
                _ => "الدراسة الجامعية",
            };
            runtime.set_focus(component.user.id, focus);
            edit_panel(component, context, "تم اختيار نوع المساعدة", "اذكر اسم المقرر والموضوع أو رقم الشريحة/الصفحة، ثم اكتب سؤالك أو أرفق ملف المحاضرة في روم الجامعة.", menu_components()).await;
        }
        _ if id.starts_with("das:focus:") => {
            let focus = id.trim_start_matches("das:focus:").to_owned();
            runtime.set_focus(component.user.id, focus.clone());
            edit_panel(component, context, "تم تحديد المسار", &format!("اختيارك: **{focus}**.\nانتقل إلى روم المختبر، ثم اكتب سؤالك أو أرفق صورة/ملفًا واضحًا. اذكر محاولتك لتحصل على شرح أدق."), menu_components()).await;
        }
        _ if id.starts_with("das:examfocus:") => {
            let focus = id.trim_start_matches("das:examfocus:").to_owned();
            runtime.set_focus(component.user.id, focus.clone());
            edit_panel(
                component,
                context,
                "تم تحديد مادة الاختبار",
                &format!("المادة المختارة: **{focus}**. اختر عدد الأسئلة."),
                subject_components("quiz-count"),
            )
            .await;
        }
        _ if id.starts_with("das:timer:start:") => {
            let minutes = id
                .trim_start_matches("das:timer:start:")
                .parse::<u64>()
                .unwrap_or(0);
            if !matches!(minutes, 25 | 40 | 50) {
                edit_panel(
                    component,
                    context,
                    "مدة غير صالحة",
                    "اختر 25 أو 40 أو 50 دقيقة.",
                    subject_components("timer"),
                )
                .await;
            } else {
                runtime.set_timer_focus(component.user.id, minutes);
                edit_panel(
                    component,
                    context,
                    "مدة التركيز",
                    &format!("اختر مدة البريك بعد كل جلسة تركيز ({minutes} دقيقة)."),
                    subject_components("timer-break"),
                )
                .await;
            }
        }
        _ if id.starts_with("das:timer:break:") => {
            let minutes = id
                .trim_start_matches("das:timer:break:")
                .parse::<u64>()
                .unwrap_or(0);
            if !matches!(minutes, 5 | 10 | 15) {
                edit_panel(
                    component,
                    context,
                    "مدة بريك غير صالحة",
                    "اختر 5 أو 10 أو 15 دقيقة.",
                    subject_components("timer-break"),
                )
                .await;
            } else {
                match runtime.set_timer_break(component.user.id, minutes) {
                    Ok(()) => {
                        edit_panel(
                            component,
                            context,
                            "عدد الجلسات",
                            &format!(
                                "تم تحديد بريك {minutes} دقائق. كم جلسة تركيز تريد في هذه الدورة؟"
                            ),
                            subject_components("timer-sessions"),
                        )
                        .await
                    }
                    Err(error) => {
                        edit_panel(
                            component,
                            context,
                            "تعذر حفظ الإعداد",
                            &error,
                            subject_components("timer"),
                        )
                        .await
                    }
                }
            }
        }
        _ if id.starts_with("das:timer:sessions:") => {
            let sessions = id
                .trim_start_matches("das:timer:sessions:")
                .parse::<u64>()
                .unwrap_or(0);
            if !(1..=4).contains(&sessions) {
                edit_panel(
                    component,
                    context,
                    "عدد جلسات غير صالح",
                    "اختر من جلسة إلى أربع جلسات.",
                    subject_components("timer-sessions"),
                )
                .await;
            } else if let Some(draft) = runtime.take_timer_draft(component.user.id) {
                match runtime.start_timer(context, component, draft.focus_minutes, draft.break_minutes, sessions).await {
                    Ok(()) => edit_panel(component, context, "بدأت دورة المذاكرة", &format!("بدأت {sessions} جلسة تركيز × {} دقيقة، وبينها بريك {} دقائق. تُحتسب 10 نقاط عند اكتمال كل جلسة تركيز، بحد أقصى 50 نقطة يوميًا.", draft.focus_minutes, draft.break_minutes), menu_components()).await,
                    Err(error) => edit_panel(component, context, "تعذر بدء الدورة", &error, subject_components("timer")).await,
                }
            } else {
                edit_panel(
                    component,
                    context,
                    "انتهت إعدادات المؤقت",
                    "ابدأ من جديد واختر مدة التركيز والبريك وعدد الجلسات.",
                    subject_components("timer"),
                )
                .await;
            }
        }
        _ if id.starts_with("das:quiz:") => {
            let count = id
                .trim_start_matches("das:quiz:")
                .parse::<u64>()
                .unwrap_or(0);
            if !matches!(count, 5 | 10) {
                edit_panel(
                    component,
                    context,
                    "عدد غير صالح",
                    "اختر 5 أو 10 أسئلة.",
                    subject_components("quiz-count"),
                )
                .await;
                return true;
            }
            if runtime.is_opted_out(component.user.id) {
                edit_panel(
                    component,
                    context,
                    "المساعدة متوقفة",
                    "فعّل المساعدة الآلية من زرها في القائمة قبل إنشاء اختبار Gemini.",
                    menu_components(),
                )
                .await;
                return true;
            }
            let focus = runtime.focus(component.user.id, component.channel_id);
            if let Err(error) = runtime.can_request(component.user.id) {
                edit_panel(
                    component,
                    context,
                    "حد الاستخدام",
                    &error,
                    subject_components("quiz-count"),
                )
                .await;
                return true;
            }
            match run_quiz(app, &focus, count).await {
                Ok(quiz) => {
                    let sent = component
                        .channel_id
                        .send_message(&context.http, CreateMessage::new().content(quiz.clone()))
                        .await;
                    if sent.is_ok() {
                        runtime.remember_quiz(component.user.id, quiz);
                    }
                    let note = if sent.is_ok() {
                        "نُشر الاختبار في الروم. أرسل إجاباتك بصيغة 1أ، 2ب، 3ج لأراجعها معك."
                    } else {
                        "تعذر نشر الاختبار. راجع صلاحية إرسال الرسائل."
                    };
                    edit_panel(component, context, "اختبارك جاهز", note, menu_components()).await;
                }
                Err(error) => {
                    edit_panel(
                        component,
                        context,
                        "تعذر إنشاء الاختبار",
                        &error,
                        subject_components("quiz-count"),
                    )
                    .await
                }
            }
        }
        _ => {
            edit_panel(
                component,
                context,
                "أداة غير معروفة",
                "استخدم /study لفتح أدوات الطالب.",
                menu_components(),
            )
            .await;
        }
    }
    true
}
fn study_channel(channel: ChannelId) -> bool {
    matches!(
        channel.get(),
        CH_CAPABILITIES | CH_TAHSEELI | CH_UNIVERSITY | CH_QUESTION_LIBRARY | CH_EXAM_ARENA
    )
}

fn is_answer_submission(text: &str) -> bool {
    let compact = text
        .to_lowercase()
        .replace(' ', "")
        .replace('،', "")
        .replace(',', "")
        .replace('.', "");
    let count = (1..=10)
        .filter(|number| {
            ['أ', 'ب', 'ج', 'د', 'a', 'b', 'c', 'd']
                .iter()
                .any(|letter| compact.contains(&format!("{number}{letter}")))
        })
        .count();
    count >= 2
}

fn should_analyze(message: &Message, bot_id: UserId, has_quiz: bool) -> bool {
    if !message.attachments.is_empty() {
        return true;
    }
    let text = message.content.to_lowercase();
    if has_quiz && is_answer_submission(&message.content) {
        return true;
    }
    if text.contains(&format!("<@{}>", bot_id.get()))
        || text.contains(&format!("<@!{}>", bot_id.get()))
    {
        return true;
    }
    [
        "؟",
        "?",
        "اشرح",
        "حل",
        "ساعدني",
        "وضح",
        "ما فهم",
        "كيف",
        "ليش",
        "سؤال",
        "راجع",
        "اختبرني",
        "صحح",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn accepted_mime(attachment: &serenity::all::Attachment) -> Option<String> {
    if let Some(value) = attachment.content_type.as_deref() {
        let mime = value.split(';').next()?.trim().to_lowercase();
        if matches!(
            mime.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/webp"
                | "application/pdf"
                | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                | "application/vnd.ms-powerpoint"
        ) {
            return Some(mime);
        }
    }
    let filename = attachment.filename.to_lowercase();
    if filename.ends_with(".png") {
        Some("image/png".to_owned())
    } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
        Some("image/jpeg".to_owned())
    } else if filename.ends_with(".webp") {
        Some("image/webp".to_owned())
    } else if filename.ends_with(".pdf") {
        Some("application/pdf".to_owned())
    } else if filename.ends_with(".pptx") {
        Some("application/vnd.openxmlformats-officedocument.presentationml.presentation".to_owned())
    } else if filename.ends_with(".ppt") {
        Some("application/vnd.ms-powerpoint".to_owned())
    } else {
        None
    }
}

async fn convert_powerpoint_to_pdf(pptx_bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let base = std::env::temp_dir().join(format!("das-pptx-{}-{unique}", std::process::id()));
        let input_path = base.with_extension("pptx");
        let output_path = base.with_extension("pdf");
        std::fs::write(&input_path, pptx_bytes)
            .map_err(|_| "تعذر تجهيز عرض PowerPoint للتحويل.".to_owned())?;

        let script = "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; $app=$null; $presentation=$null; try { $app=New-Object -ComObject PowerPoint.Application; $app.AutomationSecurity=3; $app.DisplayAlerts=0; $presentation=$app.Presentations.Open($env:DAS_PPTX_INPUT,-1,0,0); $presentation.SaveAs($env:DAS_PPTX_OUTPUT,32); $presentation.Close(); $presentation=$null; $app.Quit(); $app=$null; } finally { if($presentation){try{$presentation.Close()}catch{}}; if($app){try{$app.Quit()}catch{}} }";
        let result = Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
            .env("DAS_PPTX_INPUT", &input_path)
            .env("DAS_PPTX_OUTPUT", &output_path)
            .output()
            .map_err(|_| "تعذر تشغيل محول PowerPoint على هذا الجهاز.".to_owned())?;

        let converted = if result.status.success() {
            std::fs::read(&output_path)
                .map_err(|_| "لم ينتج PowerPoint ملف PDF صالحًا.".to_owned())
        } else {
            Err("تعذر تحويل PowerPoint إلى PDF. تأكد أن الملف سليم وغير محمي بكلمة مرور.".to_owned())
        };
        let _ = std::fs::remove_file(&input_path);
        let _ = std::fs::remove_file(&output_path);
        let bytes = converted?;
        if bytes.is_empty() || bytes.len() > MAX_FILES_BYTES {
            return Err("ملف PDF الناتج من PowerPoint فارغ أو يتجاوز 8 ميغابايت.".to_owned());
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "توقف محول PowerPoint قبل اكتمال التحويل.".to_owned())?
}

async fn fetch_attachment(
    client: &reqwest::Client,
    attachment: &serenity::all::Attachment,
) -> Result<(String, Vec<u8>), String> {
    let mime = accepted_mime(attachment)
        .ok_or_else(|| "الصيغ المدعومة: JPG وPNG وWEBP وPDF وPowerPoint (PPT/PPTX).".to_owned())?;
    if attachment.size as usize > MAX_FILE_BYTES {
        return Err("حجم الملف أكبر من 6 ميغابايت.".to_owned());
    }
    let url =
        reqwest::Url::parse(&attachment.url).map_err(|_| "رابط الملف غير صالح.".to_owned())?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com") | Some("media.discordapp.net")
        )
    {
        return Err("مصدر الملف غير مسموح.".to_owned());
    }
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "تعذر تنزيل الملف من Discord.".to_owned())?;
    if !response.status().is_success() {
        return Err("تعذر الوصول إلى الملف المرفق.".to_owned());
    }
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_FILE_BYTES)
    {
        return Err("حجم الملف أكبر من 6 ميغابايت.".to_owned());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| "تعذر قراءة الملف المرفق.".to_owned())?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err("الملف فارغ أو أكبر من الحد المسموح.".to_owned());
    }
    if mime == "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        || mime == "application/vnd.ms-powerpoint"
    {
        let pdf = convert_powerpoint_to_pdf(bytes.to_vec()).await?;
        return Ok(("application/pdf".to_owned(), pdf));
    }
    Ok((mime, bytes.to_vec()))
}
async fn call_ai(app: &AppHandle, parts: Vec<crate::ai::AIInputPart>) -> Result<String, String> {
    crate::ai::ModelRouter::generate(app, parts).await
}

async fn run_quiz(app: &AppHandle, focus: &str, count: u64) -> Result<String, String> {
    let prompt = crate::ai::prompts::PromptManager::quiz(focus, count);
    call_ai(app, vec![crate::ai::AIInputPart::Text(prompt)]).await
}

pub async fn handle_message(
    app: &AppHandle,
    runtime: &StudyRuntime,
    context: &Context,
    message: &Message,
    bot_id: UserId,
) {
    if message.author.bot || message.webhook_id.is_some() || !study_channel(message.channel_id) {
        return;
    }

    let command = message.content.trim().to_lowercase();
    if command == "!das-off" || command == "!das-stop" {
        runtime.set_opted_out(message.author.id, true);
        let _ = message.reply(&context.http, "تم إيقاف المساعدة الآلية لك. لن أرسل رسائلك أو ملفاتك إلى Gemini. لإعادة التفعيل اكتب !das-on أو استخدم /study.").await;
        return;
    }
    if command == "!das-on" {
        runtime.set_opted_out(message.author.id, false);
        let _ = message
            .reply(
                &context.http,
                "تم تفعيل المساعدة الآلية لك. أرسل سؤالًا أو ملفًا في قنوات الدراسة وسأحلله مباشرة.",
            )
            .await;
        return;
    }
    if runtime.is_opted_out(message.author.id) {
        return;
    }
    if !should_analyze(message, bot_id, runtime.has_quiz(message.author.id)) {
        return;
    }

    if let Err(error) = crate::ai::ModelRouter::is_ready(app) {
        let _ = message.reply(&context.http, error).await;
        return;
    }

    if runtime.mark_privacy_notice(message.author.id) {
        let _ = message.reply(
            &context.http,
            "تنويه لمرة واحدة: عند سؤالك أو رفع ملف في قنوات الدراسة، يُرسل محتوى الطلب إلى Google Gemini لمعالجته والرد عليك. لا ترسل بيانات شخصية أو ملفات خاصة. يمكنك إيقاف المساعدة الآلية في أي وقت بزرها في /study أو بكتابة !das-off.",
        ).await;
    }

    if let Err(error) = runtime.can_request(message.author.id) {
        let _ = message.reply(&context.http, error).await;
        return;
    }

    let focus = runtime.focus(message.author.id, message.channel_id);
    let quiz_context = if is_answer_submission(&message.content) {
        runtime.take_quiz(message.author.id)
    } else {
        None
    };
    let instruction = crate::ai::prompts::PromptManager::student_help(&focus);
    let student_text = crate::ai::context::ContextManager::limit_chars(&message.content, 6000);
    let mut parts = vec![crate::ai::AIInputPart::Text(format!(
        "{instruction}\n\nرسالة الطالب:\n{student_text}"
    ))];

    if let Some(quiz) = quiz_context {
        parts.push(crate::ai::AIInputPart::Text(format!(
            "الاختبار السابق الذي أنشأته لهذا الطالب: {quiz}\n\
             الرسالة الحالية هي إجابات الطالب. قارنها بالأسئلة، احسب الدرجة، وصحح الأخطاء واشرحها."
        )));
    }

    if !message.attachments.is_empty() {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
        {
            Ok(client) => client,
            Err(_) => {
                let _ = message
                    .reply(&context.http, "تعذر تهيئة تنزيل الملف. حاول لاحقًا.")
                    .await;
                return;
            }
        };
        let mut total = 0usize;
        for attachment in message.attachments.iter().take(2) {
            match fetch_attachment(&client, attachment).await {
                Ok((mime, bytes)) => {
                    total += bytes.len();
                    if total > MAX_FILES_BYTES {
                        let _ = message.reply(
                            &context.http,
                            "إجمالي الملفات بعد معالجة PowerPoint أكبر من 8 ميغابايت. أرسل ملفًا واحدًا أو قسّم المحتوى.",
                        ).await;
                        return;
                    }
                    parts.push(crate::ai::AIInputPart::File {
                        mime_type: mime,
                        bytes,
                    });
                }
                Err(error) => {
                    let _ = message.reply(&context.http, error).await;
                    return;
                }
            }
        }
        if message.attachments.len() > 2 {
            parts.push(crate::ai::AIInputPart::Text(
                "تمت معالجة أول ملفين فقط. أرسل بقية الملفات في رسالة أخرى.".to_owned(),
            ));
        }
    }

    let _ = message.channel_id.broadcast_typing(&context.http).await;
    match call_ai(app, parts).await {
        Ok(answer) => {
            let answer = answer.chars().take(1850).collect::<String>();
            let _ = message.reply(&context.http, answer).await;
        }
        Err(error) => {
            let _ = message.reply(&context.http, error).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::room_colour;

    #[test]
    fn each_student_area_has_a_distinct_embed_colour() {
        assert_eq!(room_colour("das:study:abilities"), 0x3B82F6);
        assert_eq!(room_colour("das:study:tahseeli"), 0x22C55E);
        assert_eq!(room_colour("das:study:university"), 0x8B5CF6);
        assert_eq!(room_colour("das:study:exam"), 0xF59E0B);
        assert_eq!(room_colour("das:study:timer"), 0x06B6D4);
        assert_eq!(room_colour("das:study:board"), 0xEAB308);
    }
}

#[cfg(test)]
mod additional_tests {
    use super::{
        is_answer_submission, study_channel, CH_CAPABILITIES, CH_EXAM_ARENA, CH_QUESTION_LIBRARY,
        CH_TAHSEELI, CH_UNIVERSITY,
    };
    use serenity::all::ChannelId;

    #[test]
    fn study_channel_allowlist_contains_only_configured_das_rooms() {
        assert!(study_channel(ChannelId::new(CH_CAPABILITIES)));
        assert!(study_channel(ChannelId::new(CH_TAHSEELI)));
        assert!(study_channel(ChannelId::new(CH_UNIVERSITY)));
        assert!(study_channel(ChannelId::new(CH_QUESTION_LIBRARY)));
        assert!(study_channel(ChannelId::new(CH_EXAM_ARENA)));
        assert!(!study_channel(ChannelId::new(1)));
    }

    #[test]
    fn quiz_answer_detector_requires_multiple_answers() {
        assert!(is_answer_submission("1أ، 2ب، 3ج"));
        assert!(is_answer_submission("1A 2B"));
        assert!(!is_answer_submission("سؤال 1"));
    }
}