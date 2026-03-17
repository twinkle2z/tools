use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use tokio::sync::mpsc::{self, Receiver, Sender, error::TryRecvError};

const RENDER_INTERVAL: Duration = Duration::from_secs(2);
const EVENT_QUEUE_CAPACITY: usize = 4096;
const MAX_MESSAGE_ROWS: usize = 20;

#[derive(Clone)]
pub struct ConnectionLog {
    next_id: Arc<AtomicUsize>,
    events: Sender<LogEvent>,
    dropped_events: Arc<AtomicU64>,
}

impl Default for ConnectionLog {
    fn default() -> Self {
        let (events, rx) = mpsc::channel(EVENT_QUEUE_CAPACITY);
        let dropped_events = Arc::new(AtomicU64::new(0));
        spawn_render_thread(rx, dropped_events.clone());
        Self {
            next_id: Arc::new(AtomicUsize::new(0)),
            events,
            dropped_events,
        }
    }
}

struct ConnectionLogState {
    active_entries: BTreeMap<usize, ActiveEntry>,
    recent_messages: VecDeque<String>,
    rendered_rows: usize,
    dirty: bool,
}

impl Default for ConnectionLogState {
    fn default() -> Self {
        Self {
            active_entries: BTreeMap::new(),
            recent_messages: VecDeque::new(),
            rendered_rows: 0,
            dirty: false,
        }
    }
}

struct ActiveEntry {
    snapshot: Arc<ConnectionSnapshot>,
    uploaded: u64,
    downloaded: u64,
}

struct ConnectionSnapshot {
    id: usize,
    prefix: String,
    uploaded: Arc<AtomicU64>,
    downloaded: Arc<AtomicU64>,
    closed: AtomicBool,
}

pub struct ConnectionHandle {
    snapshot: Arc<ConnectionSnapshot>,
}

enum LogEvent {
    Start(Arc<ConnectionSnapshot>),
    Message(String),
}

impl ConnectionLog {
    pub fn start(
        &self,
        prefix: String,
        uploaded: Arc<AtomicU64>,
        downloaded: Arc<AtomicU64>,
    ) -> ConnectionHandle {
        let snapshot = Arc::new(ConnectionSnapshot {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            prefix,
            uploaded,
            downloaded,
            closed: AtomicBool::new(false),
        });
        self.push_event(LogEvent::Start(snapshot.clone()));
        ConnectionHandle { snapshot }
    }

    pub fn print_message(&self, message: &str) {
        self.push_event(LogEvent::Message(message.to_owned()));
    }

    fn push_event(&self, event: LogEvent) {
        if self.events.try_send(event).is_err() {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl ConnectionHandle {
    pub fn close(&self, uploaded: u64, downloaded: u64) {
        self.snapshot.uploaded.store(uploaded, Ordering::Relaxed);
        self.snapshot
            .downloaded
            .store(downloaded, Ordering::Relaxed);
        self.snapshot.closed.store(true, Ordering::Release);
    }
}

fn spawn_render_thread(
    rx: Receiver<LogEvent>,
    dropped_events: Arc<AtomicU64>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("connection-log-renderer".to_owned())
        .spawn(move || run_renderer(rx, dropped_events))
        .expect("failed to spawn connection log renderer thread")
}

fn run_renderer(mut rx: Receiver<LogEvent>, dropped_events: Arc<AtomicU64>) {
    let mut state = ConnectionLogState::default();

    loop {
        thread::sleep(RENDER_INTERVAL);
        let disconnected = drain_events(&mut rx, &mut state);
        push_dropped_event_notice(&dropped_events, &mut state);
        refresh_active_entries(&mut state);

        if state.dirty {
            render_dashboard(&mut state);
        }

        if disconnected {
            return;
        }
    }
}

fn drain_events(rx: &mut Receiver<LogEvent>, state: &mut ConnectionLogState) -> bool {
    loop {
        let event = match rx.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => return true,
        };

        match event {
            LogEvent::Start(snapshot) => {
                let id = snapshot.id;
                let uploaded = snapshot.uploaded.load(Ordering::Relaxed);
                let downloaded = snapshot.downloaded.load(Ordering::Relaxed);
                state.active_entries.insert(
                    id,
                    ActiveEntry {
                        snapshot,
                        uploaded,
                        downloaded,
                    },
                );
                state.dirty = true;
            }
            LogEvent::Message(message) => {
                push_message(state, message);
            }
        }
    }
}

fn push_dropped_event_notice(dropped_events: &AtomicU64, state: &mut ConnectionLogState) {
    let dropped = dropped_events.swap(0, Ordering::Relaxed);
    if dropped > 0 {
        push_message(state, format!("[log] dropped {dropped} events"));
    }
}

fn refresh_active_entries(state: &mut ConnectionLogState) {
    let mut closed_ids = Vec::new();
    let mut changed = false;

    for (id, entry) in &mut state.active_entries {
        let uploaded = entry.snapshot.uploaded.load(Ordering::Relaxed);
        let downloaded = entry.snapshot.downloaded.load(Ordering::Relaxed);
        if uploaded != entry.uploaded || downloaded != entry.downloaded {
            entry.uploaded = uploaded;
            entry.downloaded = downloaded;
            changed = true;
        }

        if entry.snapshot.closed.load(Ordering::Acquire) {
            closed_ids.push(*id);
            changed = true;
        }
    }

    for id in closed_ids {
        state.active_entries.remove(&id);
    }

    if changed {
        state.dirty = true;
    }
}

fn push_message(state: &mut ConnectionLogState, message: String) {
    if state.recent_messages.len() == MAX_MESSAGE_ROWS {
        state.recent_messages.pop_front();
    }
    state.recent_messages.push_back(message);
    state.dirty = true;
}

fn render_dashboard(state: &mut ConnectionLogState) {
    let mut stdout = io::stdout().lock();
    if state.rendered_rows > 0 {
        let _ = write!(stdout, "\x1b[{}A", state.rendered_rows);
    }

    let mut rows = 0;
    for entry in state.active_entries.values() {
        let _ = writeln!(
            stdout,
            "\x1b[2K\r{}  up:{} down:{}",
            entry.snapshot.prefix,
            format_bytes(entry.uploaded),
            format_bytes(entry.downloaded),
        );
        rows += 1;
    }

    for message in &state.recent_messages {
        let _ = writeln!(stdout, "\x1b[2K\r{message}");
        rows += 1;
    }

    for _ in rows..state.rendered_rows {
        let _ = writeln!(stdout, "\x1b[2K\r");
    }

    state.rendered_rows = rows;
    state.dirty = false;
    let _ = stdout.flush();
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes}{}", UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}
