//! Trigger keys, read from `/dev/input/event*`.
//!
//! Devices are never grabbed: the game still receives the key and can use it
//! for its own push-to-talk. Only devices that have one of the trigger keys are
//! opened, and every other key is discarded immediately. New devices are
//! picked up as soon as they are connected.

use evdev::{Device, EventSummary, KeyCode};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

const INPUT_DIR: &str = "/dev/input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The first trigger key went down.
    Pressed,
    /// The last held trigger key was released.
    Released,
    /// A key pressed while learning.
    Learned(KeyCode),
    /// Devices that are watched, and whether input devices are readable at all.
    Devices { watched: usize, readable: bool },
}

enum Control {
    Keys(Vec<KeyCode>),
    Learn(bool),
}

#[derive(Clone)]
pub struct Listener {
    control: Sender<Control>,
}

impl Listener {
    pub fn spawn(keys: Vec<KeyCode>, on_event: impl Fn(Event) + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("input".into())
            .spawn(move || Worker::new(keys, rx, on_event).run())
            .expect("spawn input thread");
        Self { control: tx }
    }

    pub fn set_keys(&self, keys: Vec<KeyCode>) {
        let _ = self.control.send(Control::Keys(keys));
    }

    /// While learning, all keyboards and mice are read and the next key
    /// press is reported as [`Event::Learned`].
    pub fn learn(&self, on: bool) {
        let _ = self.control.send(Control::Learn(on));
    }
}

struct Open {
    path: PathBuf,
    dev: Device,
}

struct Worker<F> {
    keys: Vec<KeyCode>,
    learning: bool,
    control: Receiver<Control>,
    on_event: F,
    devices: Vec<Open>,
    held: BTreeSet<(PathBuf, u16)>,
    rescan_at: Option<Instant>,
    inotify: Option<i32>,
    last_report: Option<(usize, bool)>,
}

impl<F: Fn(Event)> Worker<F> {
    fn new(keys: Vec<KeyCode>, control: Receiver<Control>, on_event: F) -> Self {
        Self {
            keys,
            learning: false,
            control,
            on_event,
            devices: Vec::new(),
            held: BTreeSet::new(),
            rescan_at: Some(Instant::now()),
            inotify: watch_input_dir(),
            last_report: None,
        }
    }

    fn run(mut self) {
        loop {
            match self.control.try_recv() {
                Ok(Control::Keys(k)) => {
                    self.keys = k;
                    self.rescan_at = Some(Instant::now());
                }
                Ok(Control::Learn(on)) => {
                    self.learning = on;
                    self.rescan_at = Some(Instant::now());
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return,
            }
            if self.rescan_at.is_some_and(|t| Instant::now() >= t) {
                self.rescan_at = None;
                self.rescan();
            }
            self.poll();
        }
    }

    /// Opens every device that has a trigger key (or every keyboard and
    /// mouse while learning) and closes the rest.
    fn rescan(&mut self) {
        let wanted: BTreeSet<KeyCode> = self.keys.iter().copied().collect();
        let mut readable = false;
        let mut keep = Vec::new();
        let mut paths = BTreeSet::new();
        for (path, dev) in evdev::enumerate() {
            readable = true;
            let Some(supported) = dev.supported_keys() else {
                continue;
            };
            // While learning, any device with keys counts, including macro
            // pads and game controllers.
            let relevant = if self.learning {
                supported.iter().any(crate::config::usable_key)
            } else {
                wanted.iter().any(|k| supported.contains(*k))
            };
            if !relevant || dev.set_nonblocking(true).is_err() {
                continue;
            }
            paths.insert(path.clone());
            keep.push(Open { path, dev });
        }
        self.devices = keep;
        // Keys of devices that disappeared are no longer held.
        let before = !self.held.is_empty();
        self.held.retain(|(p, _)| paths.contains(p));
        if before && self.held.is_empty() {
            (self.on_event)(Event::Released);
        }
        // While learning, more devices are open than are watched afterwards.
        let report = (self.devices.len(), readable);
        if !self.learning && self.last_report != Some(report) {
            self.last_report = Some(report);
            (self.on_event)(Event::Devices {
                watched: report.0,
                readable,
            });
        }
    }

    fn poll(&mut self) {
        let mut fds: Vec<libc::pollfd> = self
            .devices
            .iter()
            .map(|d| libc::pollfd {
                fd: d.dev.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        if let Some(fd) = self.inotify {
            fds.push(libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            });
        }
        // The timeout bounds the delay for configuration changes only; key
        // events wake the poll immediately.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 200) };
        if n <= 0 {
            return;
        }
        if let Some(fd) = self.inotify
            && fds.last().is_some_and(|p| p.revents != 0)
        {
            drain(fd);
            // udev applies permissions shortly after the node appears.
            self.rescan_at = Some(Instant::now() + Duration::from_millis(400));
        }
        let mut lost = false;
        let mut keys = Vec::new();
        for (open, pfd) in self.devices.iter_mut().zip(&fds) {
            if pfd.revents == 0 {
                continue;
            }
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                lost = true;
                continue;
            }
            match open.dev.fetch_events() {
                Ok(evs) => keys.extend(evs.filter_map(|ev| match ev.destructure() {
                    EventSummary::Key(_, code, value) => Some((open.path.clone(), code, value)),
                    _ => None,
                })),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => lost = true,
            }
        }
        for (path, code, value) in keys {
            self.key(&path, code, value);
        }
        if lost {
            self.rescan_at = Some(Instant::now());
        }
    }

    fn key(&mut self, path: &Path, code: KeyCode, value: i32) {
        if self.learning {
            if value == 1 && crate::config::usable_key(code) {
                self.learning = false;
                self.rescan_at = Some(Instant::now());
                (self.on_event)(Event::Learned(code));
            }
            return;
        }
        if !self.keys.contains(&code) {
            return;
        }
        let was_held = !self.held.is_empty();
        match value {
            1 => {
                self.held.insert((path.to_path_buf(), code.code()));
            }
            0 => {
                self.held.remove(&(path.to_path_buf(), code.code()));
            }
            // 2 is the key repeat of a held key.
            _ => return,
        }
        match (was_held, !self.held.is_empty()) {
            (false, true) => (self.on_event)(Event::Pressed),
            (true, false) => (self.on_event)(Event::Released),
            _ => {}
        }
    }
}

fn watch_input_dir() -> Option<i32> {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let dir = CString::new(INPUT_DIR).unwrap();
    let mask = libc::IN_CREATE | libc::IN_DELETE | libc::IN_ATTRIB;
    if unsafe { libc::inotify_add_watch(fd, dir.as_ptr(), mask) } < 0 {
        unsafe { libc::close(fd) };
        return None;
    }
    Some(fd)
}

fn drain(fd: i32) {
    let mut buf = [0u8; 4096];
    while unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
}
