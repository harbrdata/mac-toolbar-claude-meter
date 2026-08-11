use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::*;
use objc2_foundation::*;

use crate::api::{self, FetchResult, UsageWindow};
use crate::gauge;
use crate::keychain;
use crate::launch_agent;
use crate::notification;

// GCD FFI for dispatching closures back to the main thread.
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// Run a closure on the main thread via GCD `dispatch_async`.
fn dispatch_main<F: FnOnce() + Send + 'static>(f: F) {
    let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    let raw = Box::into_raw(boxed) as *mut c_void;

    extern "C" fn trampoline(context: *mut c_void) {
        // SAFETY: `context` was created by `Box::into_raw` in `dispatch_main` and is
        // guaranteed to be called exactly once by GCD's `dispatch_async`.
        let boxed: Box<Box<dyn FnOnce() + Send>> = unsafe { Box::from_raw(context as *mut _) };
        boxed();
    }

    // SAFETY: `_dispatch_main_q` is the global main queue provided by libdispatch.
    // `dispatch_async_f` takes ownership of `raw` and calls `trampoline` exactly once.
    unsafe {
        dispatch_async_f(&_dispatch_main_q as *const c_void, raw, trampoline);
    }
}

/// A `Send`-safe handle to the `AppDelegate` for use in `dispatch_main` closures.
///
/// # Safety
/// The wrapped pointer must only be dereferenced on the main thread (via `dispatch_main`).
/// The `AppDelegate` instance must outlive all `MainThreadHandle` copies — guaranteed
/// because `AppDelegate` is `mem::forget`-ed in `run()` and lives for the process lifetime.
struct MainThreadHandle(usize);

// SAFETY: The pointer is never dereferenced off the main thread. Background threads
// only carry the handle; all dereferences happen inside `dispatch_main` closures which
// execute on the main thread via GCD.
unsafe impl Send for MainThreadHandle {}

impl MainThreadHandle {
    fn new(delegate: &AppDelegate) -> Self {
        Self(delegate as *const AppDelegate as usize)
    }

    /// Recover the `AppDelegate` reference. Must only be called on the main thread.
    ///
    /// # Safety
    /// Caller must be on the main thread and the `AppDelegate` must still be alive.
    unsafe fn get(&self) -> &AppDelegate {
        unsafe { &*(self.0 as *const AppDelegate) }
    }
}

const ICON_SIZE: f64 = 24.0;
const POLL_INTERVAL_DEFAULT: f64 = 120.0;
const POLL_INTERVAL_OPTIONS: &[u64] = &[60, 120, 300, 600];
const RATE_LIMIT_PAUSE_DEFAULT: u64 = 60;
const RATE_LIMIT_PAUSE_MAX: u64 = 600;
const LOG_CAPACITY: usize = 20;
const ALERT_THRESHOLD_DEFAULT: f64 = 0.95;
const ALERT_THRESHOLD_OPTIONS: &[u64] = &[75, 80, 85, 90, 95, 100];
const ALERT_THRESHOLD_7D_DEFAULT: f64 = 0.80;
const SHOW_BOTH_WINDOWS_DEFAULT: bool = false;
/// Icon width multiplier used for paused/error icons when dual-window mode is on,
/// matching the width of `create_dual_gauge_icon`'s output at the same `ICON_SIZE`.
const DUAL_WIDTH_MULT: f64 = 2.15;

/// Persistable user preferences, backed by `NSUserDefaults`.
struct Preferences {
    poll_interval: f64,
    alert_threshold: f64,
    alert_threshold_7d: f64,
    polling_enabled: bool,
    show_both_windows: bool,
}

/// Outcome of comparing a window's utilization against its alert threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertDecision {
    Fire,
    Reset,
    None,
}

/// Pure decision logic for whether an alert should fire, reset, or do nothing.
///
/// `threshold > 1.0` means alerts are disabled for this window. Otherwise: fire
/// when usage crosses at/above the threshold and hasn't already fired; reset the
/// fired flag once usage drops back below the threshold.
fn alert_decision(util: f64, threshold: f64, already_fired: bool) -> AlertDecision {
    if threshold > 1.0 {
        return AlertDecision::None;
    }
    if util >= threshold && !already_fired {
        AlertDecision::Fire
    } else if util < threshold && already_fired {
        AlertDecision::Reset
    } else {
        AlertDecision::None
    }
}

/// Find the usage window with the given label, if present.
fn find_window(windows: &[UsageWindow], label: &str) -> Option<UsageWindow> {
    windows.iter().find(|w| w.label == label).cloned()
}

/// True when the primary window is itself the 7d window. The primary falls back to the
/// first window returned by the API when no 5h window is present, so the two can coincide;
/// callers must then avoid treating 7d as a distinct second window.
fn primary_is_seven_day(primary: &Option<UsageWindow>) -> bool {
    primary.as_ref().is_some_and(|w| w.label == "7d")
}

/// Which threshold/fired-flag pair governs a window being checked for an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertSlot {
    FiveHour,
    SevenDay,
}

/// Pair each window that needs an alert check with the slot that governs it.
///
/// Normally that is the primary window on the 5h slot plus the 7d window on the 7d slot.
/// When the API omits the 5h window the primary falls back to the first window, which may
/// itself be the 7d one — that window must then be governed by the 7d threshold, and must
/// not also be checked a second time via the 7d slot.
fn alert_checks(
    primary: &Option<UsageWindow>,
    windows: &[UsageWindow],
) -> Vec<(UsageWindow, AlertSlot)> {
    let mut checks: Vec<(UsageWindow, AlertSlot)> = Vec::new();
    if let Some(p) = primary.clone() {
        let slot = if p.label == "7d" {
            AlertSlot::SevenDay
        } else {
            AlertSlot::FiveHour
        };
        checks.push((p, slot));
    }
    let seven_day_covered = checks.iter().any(|(_, s)| *s == AlertSlot::SevenDay);
    if !seven_day_covered && let Some(w) = find_window(windows, "7d") {
        checks.push((w, AlertSlot::SevenDay));
    }
    checks
}

/// Which icon variant `refresh_icon` should draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconMode {
    Normal,
    Paused,
    Error,
}

/// Ivars wrapper: `fetch_in_progress` is a `Cell<bool>` so it can be checked/set
/// without borrowing the full `AppState` through the `RefCell`, eliminating a class
/// of potential borrow panics.
pub struct AppIvars {
    state: RefCell<AppState>,
    fetch_in_progress: Cell<bool>,
}

pub struct AppState {
    mtm: MainThreadMarker,
    status_item: Option<Retained<NSStatusItem>>,
    menu: Option<Retained<NSMenu>>,
    poll_interval: f64,
    poll_timer: Option<Retained<NSTimer>>,
    polling_enabled: bool,
    last_windows: Vec<UsageWindow>,
    last_primary: Option<UsageWindow>,
    rate_limited: bool,
    rate_limit_resume: Option<Instant>,
    rate_limit_backoff: u32,
    rate_limit_timer: Option<Retained<NSTimer>>,
    rate_limit_countdown_timer: Option<Retained<NSTimer>>,
    cached_token: Option<String>,
    cached_token_expires: Option<std::time::Instant>,
    cached_creds_fingerprint: Option<String>,
    log_buffer: Vec<String>,
    log_write_count: u32,
    alert_threshold: f64,
    alert_fired: bool,
    alert_threshold_7d: f64,
    alert_fired_7d: bool,
    show_both_windows: bool,
}

impl AppState {
    /// Snapshot the persistable subset of state into a `Preferences` for `save_preferences`.
    fn to_preferences(&self) -> Preferences {
        Preferences {
            poll_interval: self.poll_interval,
            alert_threshold: self.alert_threshold,
            alert_threshold_7d: self.alert_threshold_7d,
            polling_enabled: self.polling_enabled,
            show_both_windows: self.show_both_windows,
        }
    }

    fn push_log(&mut self, msg: String) {
        if self.log_buffer.len() >= LOG_CAPACITY {
            self.log_buffer.remove(0);
        }
        // Append to log file
        let log_path = launch_agent::log_file_path();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = writeln!(f, "{}", msg);
        }
        // Check for rotation every 100 writes
        self.log_write_count += 1;
        if self.log_write_count >= 100 {
            self.log_write_count = 0;
            launch_agent::rotate_log_if_needed();
        }
        self.log_buffer.push(msg);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppDelegate"]
    #[ivars = AppIvars]
    pub struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.setup();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            // Invalidate all timers to prevent them firing during teardown.
            // AppState is inside a mem::forget-ed delegate so Drop never runs.
            let state = self.ivars().state.borrow();
            if let Some(ref timer) = state.poll_timer {
                timer.invalidate();
            }
            if let Some(ref timer) = state.rate_limit_timer {
                timer.invalidate();
            }
            if let Some(ref timer) = state.rate_limit_countdown_timer {
                timer.invalidate();
            }
            drop(state);
            launch_agent::cleanup_if_uninstalled();
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            // If the .app bundle was deleted, clean up and quit
            if !std::path::Path::new("/Applications/Claude-o-Meter.app").exists() {
                launch_agent::cleanup_if_uninstalled();
                let state = self.ivars().state.borrow();
                let app = NSApplication::sharedApplication(state.mtm);
                app.terminate(None);
                return;
            }

            let state = self.ivars().state.borrow();
            if !state.polling_enabled || state.rate_limited {
                return;
            }
            drop(state);
            self.fetch_and_update();
        }

        #[unsafe(method(refresh:))]
        fn refresh(&self, _sender: &AnyObject) {
            self.clear_rate_limit();
            self.fetch_and_update();
        }

        #[unsafe(method(togglePolling:))]
        fn toggle_polling(&self, _sender: &AnyObject) {
            let mut state = self.ivars().state.borrow_mut();
            state.polling_enabled = !state.polling_enabled;

            if state.polling_enabled {
                state.push_log(format!("{} Polling enabled", timestamp()));
                let interval = state.poll_interval;
                drop(state);
                self.start_timer(interval);
                self.refresh_icon(IconMode::Normal);
            } else {
                state.push_log(format!("{} Polling disabled", timestamp()));
                if let Some(ref timer) = state.poll_timer {
                    timer.invalidate();
                }
                state.poll_timer = None;
                drop(state);
                self.refresh_icon(IconMode::Paused);
            }
            let state = self.ivars().state.borrow();
            let prefs = state.to_preferences();
            drop(state);
            save_preferences(&prefs);
            self.rebuild_menu();
        }

        #[unsafe(method(setInterval60:))]
        fn set_interval_60(&self, _sender: &AnyObject) { self.set_interval(60.0); }
        #[unsafe(method(setInterval120:))]
        fn set_interval_120(&self, _sender: &AnyObject) { self.set_interval(120.0); }
        #[unsafe(method(setInterval300:))]
        fn set_interval_300(&self, _sender: &AnyObject) { self.set_interval(300.0); }
        #[unsafe(method(setInterval600:))]
        fn set_interval_600(&self, _sender: &AnyObject) { self.set_interval(600.0); }

        #[unsafe(method(setAlert75:))]
        fn set_alert_75(&self, _sender: &AnyObject) { self.set_alert_threshold(0.75); }
        #[unsafe(method(setAlert80:))]
        fn set_alert_80(&self, _sender: &AnyObject) { self.set_alert_threshold(0.80); }
        #[unsafe(method(setAlert85:))]
        fn set_alert_85(&self, _sender: &AnyObject) { self.set_alert_threshold(0.85); }
        #[unsafe(method(setAlert90:))]
        fn set_alert_90(&self, _sender: &AnyObject) { self.set_alert_threshold(0.90); }
        #[unsafe(method(setAlert95:))]
        fn set_alert_95(&self, _sender: &AnyObject) { self.set_alert_threshold(0.95); }
        #[unsafe(method(setAlert100:))]
        fn set_alert_100(&self, _sender: &AnyObject) { self.set_alert_threshold(1.01); }

        #[unsafe(method(setAlert7d75:))]
        fn set_alert_7d_75(&self, _sender: &AnyObject) { self.set_alert_threshold_7d(0.75); }
        #[unsafe(method(setAlert7d80:))]
        fn set_alert_7d_80(&self, _sender: &AnyObject) { self.set_alert_threshold_7d(0.80); }
        #[unsafe(method(setAlert7d85:))]
        fn set_alert_7d_85(&self, _sender: &AnyObject) { self.set_alert_threshold_7d(0.85); }
        #[unsafe(method(setAlert7d90:))]
        fn set_alert_7d_90(&self, _sender: &AnyObject) { self.set_alert_threshold_7d(0.90); }
        #[unsafe(method(setAlert7d95:))]
        fn set_alert_7d_95(&self, _sender: &AnyObject) { self.set_alert_threshold_7d(0.95); }
        #[unsafe(method(setAlert7d100:))]
        fn set_alert_7d_100(&self, _sender: &AnyObject) { self.set_alert_threshold_7d(1.01); }

        #[unsafe(method(toggleShowBoth:))]
        fn toggle_show_both_action(&self, _sender: &AnyObject) { self.toggle_show_both(); }

        #[unsafe(method(toggleLoginItem:))]
        fn toggle_login_item(&self, _sender: &AnyObject) {
            if launch_agent::is_enabled() {
                launch_agent::disable();
            } else {
                launch_agent::enable();
            }
            self.rebuild_menu();
        }

        #[unsafe(method(rateLimitResume:))]
        fn rate_limit_resume(&self, _timer: &NSTimer) {
            let mut state = self.ivars().state.borrow_mut();
            state.push_log(format!("{} Rate-limit pause expired, resuming", timestamp()));
            drop(state);
            self.clear_rate_limit();
            self.fetch_and_update();
        }

        #[unsafe(method(rateLimitCountdown:))]
        fn rate_limit_countdown(&self, _timer: &NSTimer) {
            let state = self.ivars().state.borrow();
            if state.rate_limited {
                drop(state);
                self.rebuild_menu();
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            let state = self.ivars().state.borrow();
            let app = NSApplication::sharedApplication(state.mtm);
            app.terminate(None);
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // Load saved preferences
        let prefs = load_preferences();

        let this = mtm.alloc::<AppDelegate>();
        let this = this.set_ivars(AppIvars {
            state: RefCell::new(AppState {
                mtm,
                status_item: None,
                menu: None,
                poll_interval: prefs.poll_interval,
                poll_timer: None,
                polling_enabled: prefs.polling_enabled,
                last_windows: Vec::new(),
                last_primary: None,
                rate_limited: false,
                rate_limit_resume: None,
                rate_limit_backoff: 0,
                rate_limit_timer: None,
                rate_limit_countdown_timer: None,
                cached_token: None,
                cached_token_expires: None,
                cached_creds_fingerprint: None,
                log_buffer: Vec::new(),
                log_write_count: 0,
                alert_threshold: prefs.alert_threshold,
                alert_fired: false,
                alert_threshold_7d: prefs.alert_threshold_7d,
                alert_fired_7d: false,
                show_both_windows: prefs.show_both_windows,
            }),
            fetch_in_progress: Cell::new(false),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn mtm(&self) -> MainThreadMarker {
        self.ivars().state.borrow().mtm
    }

    fn setup(&self) {
        let _ = std::fs::create_dir_all(launch_agent::log_dir());
        launch_agent::rotate_log_if_needed();
        notification::request_authorization();

        let mtm = self.mtm();
        unsafe {
            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);

            if let Some(button) = status_item.button(mtm) {
                button.setImage(Some(&gauge::create_gauge_icon(0.0, None, ICON_SIZE)));
                button.setTitle(&NSString::from_str(""));
            }

            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            status_item.setMenu(Some(&menu));

            {
                let mut state = self.ivars().state.borrow_mut();
                state.status_item = Some(status_item);
                state.menu = Some(menu);
                state.push_log(format!("{} Claude Meter starting...", timestamp()));
            }

            self.rebuild_menu();

            let this: Retained<NSObject> = Retained::into_super(self.retain());
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                2.0,
                &this,
                sel!(tick:),
                None,
                false,
            );

            let state = self.ivars().state.borrow();
            let saved_interval = state.poll_interval;
            let polling = state.polling_enabled;
            drop(state);

            if polling {
                self.start_timer(saved_interval);
            } else {
                self.refresh_icon(IconMode::Paused);
            }
        }
    }

    fn start_timer(&self, interval: f64) {
        let mut state = self.ivars().state.borrow_mut();
        if let Some(ref timer) = state.poll_timer {
            timer.invalidate();
        }
        state.poll_interval = interval;
        unsafe {
            let this: Retained<NSObject> = Retained::into_super(self.retain());
            state.poll_timer = Some(
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    interval,
                    &this,
                    sel!(tick:),
                    None,
                    true,
                ),
            );
        }
    }

    fn set_alert_threshold(&self, threshold: f64) {
        let mut state = self.ivars().state.borrow_mut();
        state.alert_threshold = threshold;
        state.alert_fired = false;
        if threshold > 1.0 {
            state.push_log(format!("{} Usage alert disabled", timestamp()));
        } else {
            state.push_log(format!(
                "{} Alert threshold set to {}%",
                timestamp(),
                (threshold * 100.0) as u32
            ));
        }
        let prefs = state.to_preferences();
        drop(state);
        save_preferences(&prefs);
        // Immediately check if current usage exceeds the new threshold
        self.check_and_fire_alert();
        self.rebuild_menu();
    }

    fn set_alert_threshold_7d(&self, threshold: f64) {
        let mut state = self.ivars().state.borrow_mut();
        state.alert_threshold_7d = threshold;
        state.alert_fired_7d = false;
        if threshold > 1.0 {
            state.push_log(format!("{} 7d usage alert disabled", timestamp()));
        } else {
            state.push_log(format!(
                "{} 7d alert threshold set to {}%",
                timestamp(),
                (threshold * 100.0) as u32
            ));
        }
        let prefs = state.to_preferences();
        drop(state);
        save_preferences(&prefs);
        // Immediately check if current usage exceeds the new threshold
        self.check_and_fire_alert();
        self.rebuild_menu();
    }

    /// Check both the 5h and 7d windows against their respective alert
    /// thresholds/fired-flags, firing or resetting each independently.
    fn check_and_fire_alert(&self) {
        let mut state = self.ivars().state.borrow_mut();
        let checks = alert_checks(&state.last_primary, &state.last_windows);

        let mut to_notify: Vec<(String, String)> = Vec::new();
        for (window, slot) in checks {
            let (threshold, already_fired) = match slot {
                AlertSlot::FiveHour => (state.alert_threshold, state.alert_fired),
                AlertSlot::SevenDay => (state.alert_threshold_7d, state.alert_fired_7d),
            };
            match alert_decision(window.utilization, threshold, already_fired) {
                AlertDecision::Fire => {
                    match slot {
                        AlertSlot::FiveHour => state.alert_fired = true,
                        AlertSlot::SevenDay => state.alert_fired_7d = true,
                    }
                    let pct = (window.utilization * 100.0) as u32;
                    state.push_log(format!(
                        "{} Alert: {} usage at {}%",
                        timestamp(),
                        window.label,
                        pct
                    ));
                    to_notify.push((
                        window.label.to_string(),
                        format!("{} window usage is at {}%", window.label, pct),
                    ));
                }
                AlertDecision::Reset => match slot {
                    AlertSlot::FiveHour => state.alert_fired = false,
                    AlertSlot::SevenDay => state.alert_fired_7d = false,
                },
                AlertDecision::None => {}
            }
        }
        drop(state);

        for (id, body) in &to_notify {
            notification::post(id, "Claude Meter", "Usage alert", body);
        }
    }

    fn set_interval(&self, seconds: f64) {
        let mut state = self.ivars().state.borrow_mut();
        state.push_log(format!(
            "{} Poll interval changed to {}s",
            timestamp(),
            seconds as u64
        ));
        state.poll_interval = seconds;
        let prefs = state.to_preferences();
        drop(state);
        self.start_timer(seconds);
        save_preferences(&prefs);
        self.rebuild_menu();
    }

    fn toggle_show_both(&self) {
        let mut state = self.ivars().state.borrow_mut();
        state.show_both_windows = !state.show_both_windows;
        let show_both = state.show_both_windows;
        state.push_log(format!(
            "{} Show both windows: {}",
            timestamp(),
            if show_both { "on" } else { "off" }
        ));
        let prefs = state.to_preferences();
        let paused = !state.polling_enabled || state.rate_limited;
        drop(state);
        save_preferences(&prefs);
        self.refresh_icon(if paused {
            IconMode::Paused
        } else {
            IconMode::Normal
        });
        self.rebuild_menu();
    }

    /// Draw and set the status-item icon for the given mode, keeping the
    /// fetch-success, pause, and error paths visually consistent (dual-gauge
    /// when both-windows mode is on and a 7d window is known, single gauge
    /// with a muted 7d underlay otherwise).
    fn refresh_icon(&self, mode: IconMode) {
        let state = self.ivars().state.borrow();
        let show_both = state.show_both_windows;
        let five_h = state
            .last_primary
            .as_ref()
            .map(|w| w.utilization)
            .unwrap_or(0.0);
        // Suppress the 7d value when the primary window already is the 7d one, so dual
        // mode can't render the same gauge twice.
        let seven_d = if primary_is_seven_day(&state.last_primary) {
            None
        } else {
            find_window(&state.last_windows, "7d").map(|w| w.utilization)
        };
        drop(state);

        // One rule for all three modes, so the status item never changes width purely
        // because it is paused or erroring. Dual width needs a 7d value to show, which is
        // absent until the first successful fetch.
        let dual = show_both && seven_d.is_some();
        let width_mult = if dual { DUAL_WIDTH_MULT } else { 1.0 };

        let icon = match mode {
            IconMode::Normal => match seven_d {
                Some(s) if dual => gauge::create_dual_gauge_icon(five_h, s, ICON_SIZE),
                _ => gauge::create_gauge_icon(five_h, seven_d, ICON_SIZE),
            },
            IconMode::Paused => gauge::create_paused_icon(ICON_SIZE, width_mult),
            IconMode::Error => gauge::create_error_icon(ICON_SIZE, width_mult),
        };
        self.set_icon(&icon);
    }

    fn set_icon(&self, icon: &NSImage) {
        let state = self.ivars().state.borrow();
        let mtm = state.mtm;
        if let Some(ref si) = state.status_item
            && let Some(button) = si.button(mtm)
        {
            button.setImage(Some(icon));
            button.setTitle(&NSString::from_str(""));
        }
    }

    fn fetch_and_update(&self) {
        if self.ivars().fetch_in_progress.get() {
            return;
        }
        self.ivars().fetch_in_progress.set(true);

        let mut state = self.ivars().state.borrow_mut();
        state.push_log(format!("{} Fetching usage data...", timestamp()));

        // Always read keychain so we detect account switches / re-logins
        // before the in-memory token's derived expiry elapses.
        let credentials = keychain::read_credentials();

        let fingerprint = credentials.as_ref().map(creds_fingerprint);
        let keychain_changed = fingerprint.is_some()
            && state.cached_creds_fingerprint.as_ref() != fingerprint.as_ref();
        if keychain_changed {
            state.push_log(format!(
                "{} Keychain credentials changed, invalidating token cache",
                timestamp()
            ));
            state.cached_token = None;
            state.cached_token_expires = None;
            state.cached_creds_fingerprint = fingerprint.clone();
        }

        let token_expired = keychain_changed
            || match state.cached_token_expires {
                Some(expires) => std::time::Instant::now() >= expires,
                None => state.cached_token.is_none(),
            };

        let cached_token = state.cached_token.clone();

        if token_expired {
            state.cached_token = None;
            state.cached_token_expires = None;
        }

        drop(state);

        if token_expired && credentials.is_none() {
            let mut state = self.ivars().state.borrow_mut();
            state.push_log(format!("{} [ERROR] No credentials found", timestamp()));
            drop(state);
            self.ivars().fetch_in_progress.set(false);
            self.show_error();
            return;
        }

        let handle = MainThreadHandle::new(self);

        std::thread::spawn(move || {
            // --- Background thread: all network I/O happens here ---
            let (token, new_token_info) = if token_expired {
                let creds = credentials.unwrap(); // safe: checked above
                match api::get_access_token(&creds) {
                    Some(result) => {
                        let info = (result.access_token.clone(), result.expires_in_secs);
                        (Some(result.access_token), Some(info))
                    }
                    None => (None, None),
                }
            } else {
                (cached_token, None)
            };

            let Some(token) = token else {
                dispatch_main(move || {
                    // SAFETY: dispatch_main runs on the main thread; AppDelegate is process-lived.
                    let app = unsafe { handle.get() };
                    let mut state = app.ivars().state.borrow_mut();
                    state.push_log(format!("{} [ERROR] No access token", timestamp()));
                    drop(state);
                    app.ivars().fetch_in_progress.set(false);
                    app.show_error();
                });
                return;
            };

            let fetch_result = api::fetch_usage(&token);

            // --- Dispatch back to main thread for UI updates ---
            dispatch_main(move || {
                // SAFETY: dispatch_main runs on the main thread; AppDelegate is process-lived.
                let app = unsafe { handle.get() };

                // Update token cache if we refreshed
                if let Some((new_token, expires_in)) = new_token_info {
                    let mut state = app.ivars().state.borrow_mut();
                    state.cached_token = Some(new_token);
                    if let Some(secs) = expires_in {
                        let buffer = secs.saturating_sub(60);
                        state.cached_token_expires = Some(
                            std::time::Instant::now() + std::time::Duration::from_secs(buffer),
                        );
                    }
                    drop(state);
                }

                match fetch_result {
                    FetchResult::Ok(data) => {
                        let windows = api::parse_usage(&data);
                        let primary = windows
                            .iter()
                            .find(|w| w.label == "5h")
                            .cloned()
                            .or_else(|| windows.first().cloned());

                        let mut state = app.ivars().state.borrow_mut();
                        state.push_log(format!(
                            "{} Got {} usage windows",
                            timestamp(),
                            windows.len()
                        ));
                        state.rate_limit_backoff = 0;
                        state.last_windows = windows;
                        state.last_primary = primary.clone();
                        drop(state);
                        app.ivars().fetch_in_progress.set(false);

                        app.refresh_icon(IconMode::Normal);
                        app.check_and_fire_alert();
                        app.rebuild_menu();
                    }
                    FetchResult::RateLimited(retry_after) => {
                        app.ivars().fetch_in_progress.set(false);
                        app.enter_rate_limit_pause(retry_after);
                    }
                    FetchResult::AuthError => {
                        let mut state = app.ivars().state.borrow_mut();
                        state.cached_token = None;
                        state.cached_token_expires = None;
                        state.push_log(format!("{} [WARN] Auth error, will retry", timestamp()));
                        drop(state);
                        app.ivars().fetch_in_progress.set(false);
                        app.show_error();
                    }
                    FetchResult::Error(e) => {
                        let mut state = app.ivars().state.borrow_mut();
                        state.cached_token = None;
                        state.cached_token_expires = None;
                        state.push_log(format!("{} [ERROR] {}", timestamp(), e));
                        drop(state);
                        app.ivars().fetch_in_progress.set(false);
                        app.show_error();
                    }
                }
            });
        });
    }

    fn show_error(&self) {
        let state = self.ivars().state.borrow();
        let have_data = !state.last_windows.is_empty();
        drop(state);
        if have_data {
            self.refresh_icon(IconMode::Normal);
        } else {
            self.refresh_icon(IconMode::Error);
        }
        self.rebuild_menu();
    }

    fn enter_rate_limit_pause(&self, retry_after: u64) {
        let mut state = self.ivars().state.borrow_mut();
        state.rate_limit_backoff += 1;

        let pause = if retry_after > 0 {
            retry_after.min(RATE_LIMIT_PAUSE_MAX)
        } else {
            (RATE_LIMIT_PAUSE_DEFAULT * 2u64.pow(state.rate_limit_backoff - 1))
                .min(RATE_LIMIT_PAUSE_MAX)
        };

        state.rate_limited = true;
        state.rate_limit_resume = Some(Instant::now() + std::time::Duration::from_secs(pause));
        let backoff = state.rate_limit_backoff;
        state.push_log(format!(
            "{} Pausing polling for {}s due to rate limit (attempt {})",
            timestamp(),
            pause,
            backoff
        ));

        if let Some(ref t) = state.rate_limit_timer {
            t.invalidate();
        }
        if let Some(ref t) = state.rate_limit_countdown_timer {
            t.invalidate();
        }

        unsafe {
            let this: Retained<NSObject> = Retained::into_super(self.retain());
            state.rate_limit_timer = Some(
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    pause as f64,
                    &this,
                    sel!(rateLimitResume:),
                    None,
                    false,
                ),
            );
            let this2: Retained<NSObject> = Retained::into_super(self.retain());
            state.rate_limit_countdown_timer = Some(
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    10.0,
                    &this2,
                    sel!(rateLimitCountdown:),
                    None,
                    true,
                ),
            );
        }
        drop(state);

        self.refresh_icon(IconMode::Paused);
        self.rebuild_menu();
    }

    fn clear_rate_limit(&self) {
        let mut state = self.ivars().state.borrow_mut();
        state.rate_limited = false;
        state.rate_limit_backoff = 0;
        state.rate_limit_resume = None;
        if let Some(ref t) = state.rate_limit_timer {
            t.invalidate();
        }
        state.rate_limit_timer = None;
        if let Some(ref t) = state.rate_limit_countdown_timer {
            t.invalidate();
        }
        state.rate_limit_countdown_timer = None;
    }

    fn rebuild_menu(&self) {
        let state = self.ivars().state.borrow();
        let Some(ref menu) = state.menu else { return };
        let mtm = state.mtm;

        {
            menu.removeAllItems();
            let mono = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 12.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(12.0));
            let mono_small = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 11.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(11.0));

            // Rate-limit banner
            if state.rate_limited
                && let Some(ref resume_at) = state.rate_limit_resume
            {
                let remaining = resume_at
                    .saturating_duration_since(Instant::now())
                    .as_secs();
                let mins = remaining / 60;
                let secs = remaining % 60;
                let banner = styled_item(
                    &format!(
                        "\u{26a0}\u{fe0f}  Rate limited \u{2014} polling paused ({mins}m {secs}s)"
                    ),
                    &mono,
                    Some(&NSColor::systemOrangeColor()),
                    mtm,
                );
                menu.addItem(&banner);
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }

            // Usage windows
            if state.last_windows.is_empty() {
                menu.addItem(&styled_item(
                    "Loading...",
                    &mono,
                    Some(&NSColor::secondaryLabelColor()),
                    mtm,
                ));
            } else {
                for w in &state.last_windows {
                    let pct = (w.utilization * 100.0) as i32;
                    let reset = api::format_reset_time(w.resets_at.as_deref());

                    let label_text = format!(" {}: {}%  ", w.label, pct);
                    let line = gradient_bar_item(&label_text, w.utilization, 20, &mono, mtm);
                    line.setImage(Some(&gauge::create_gauge_icon(w.utilization, None, 16.0)));
                    menu.addItem(&line);

                    let reset_item = styled_item(
                        &format!("       Resets in: {}", reset),
                        &mono_small,
                        None,
                        mtm,
                    );
                    menu.addItem(&reset_item);
                    menu.addItem(&NSMenuItem::separatorItem(mtm));
                }
            }

            // Actions
            let this: Retained<NSObject> = Retained::into_super(self.retain());

            menu.addItem(&action_item("Refresh Now", sel!(refresh:), &this, mtm));

            let polling_label = if state.polling_enabled {
                "Polling: On"
            } else {
                "Polling: Off"
            };
            menu.addItem(&action_item(
                polling_label,
                sel!(togglePolling:),
                &this,
                mtm,
            ));

            let show_both_label = if state.show_both_windows {
                "Show Both Windows: On"
            } else {
                "Show Both Windows: Off"
            };
            menu.addItem(&action_item(
                show_both_label,
                sel!(toggleShowBoth:),
                &this,
                mtm,
            ));

            // Interval submenu
            let interval_item = NSMenuItem::new(mtm);
            interval_item.setTitle(&NSString::from_str("Refresh Interval"));
            let interval_menu = NSMenu::new(mtm);
            let selectors = [
                sel!(setInterval60:),
                sel!(setInterval120:),
                sel!(setInterval300:),
                sel!(setInterval600:),
            ];
            for (i, &secs) in POLL_INTERVAL_OPTIONS.iter().enumerate() {
                let label = if secs < 60 {
                    format!("{secs}s")
                } else {
                    format!("{}m", secs / 60)
                };
                let opt = action_item(&label, selectors[i], &this, mtm);
                if secs as f64 == state.poll_interval {
                    opt.setState(1); // checkmark
                }
                interval_menu.addItem(&opt);
            }
            interval_item.setSubmenu(Some(&interval_menu));
            menu.addItem(&interval_item);

            // 5h alert threshold submenu
            let alert_item = NSMenuItem::new(mtm);
            alert_item.setTitle(&NSString::from_str("5h Alert Threshold"));
            let alert_menu = NSMenu::new(mtm);
            let alert_selectors = [
                sel!(setAlert75:),
                sel!(setAlert80:),
                sel!(setAlert85:),
                sel!(setAlert90:),
                sel!(setAlert95:),
                sel!(setAlert100:),
            ];
            for (i, &pct) in ALERT_THRESHOLD_OPTIONS.iter().enumerate() {
                let label = if pct >= 100 {
                    "Off".to_string()
                } else {
                    format!("{}%", pct)
                };
                let opt = action_item(&label, alert_selectors[i], &this, mtm);
                let threshold_val = if pct >= 100 { 1.01 } else { pct as f64 / 100.0 };
                if (threshold_val - state.alert_threshold).abs() < 0.001 {
                    opt.setState(1);
                }
                alert_menu.addItem(&opt);
            }
            alert_item.setSubmenu(Some(&alert_menu));
            menu.addItem(&alert_item);

            // 7d alert threshold submenu
            let alert_7d_item = NSMenuItem::new(mtm);
            alert_7d_item.setTitle(&NSString::from_str("7d Alert Threshold"));
            let alert_7d_menu = NSMenu::new(mtm);
            let alert_7d_selectors = [
                sel!(setAlert7d75:),
                sel!(setAlert7d80:),
                sel!(setAlert7d85:),
                sel!(setAlert7d90:),
                sel!(setAlert7d95:),
                sel!(setAlert7d100:),
            ];
            for (i, &pct) in ALERT_THRESHOLD_OPTIONS.iter().enumerate() {
                let label = if pct >= 100 {
                    "Off".to_string()
                } else {
                    format!("{}%", pct)
                };
                let opt = action_item(&label, alert_7d_selectors[i], &this, mtm);
                let threshold_val = if pct >= 100 { 1.01 } else { pct as f64 / 100.0 };
                if (threshold_val - state.alert_threshold_7d).abs() < 0.001 {
                    opt.setState(1);
                }
                alert_7d_menu.addItem(&opt);
            }
            alert_7d_item.setSubmenu(Some(&alert_7d_menu));
            menu.addItem(&alert_7d_item);

            // Login item toggle
            let login_label = if launch_agent::is_enabled() {
                "Start at Login: On"
            } else {
                "Start at Login: Off"
            };
            menu.addItem(&action_item(
                login_label,
                sel!(toggleLoginItem:),
                &this,
                mtm,
            ));

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Logs submenu
            let logs_item = NSMenuItem::new(mtm);
            logs_item.setTitle(&NSString::from_str("Recent Logs"));
            let logs_menu = NSMenu::new(mtm);
            logs_menu.setAutoenablesItems(false);
            let log_font = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 10.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(10.0));
            if state.log_buffer.is_empty() {
                logs_menu.addItem(&styled_item(
                    "(no logs yet)",
                    &log_font,
                    Some(&NSColor::secondaryLabelColor()),
                    mtm,
                ));
            } else {
                let start = state.log_buffer.len().saturating_sub(10);
                for line in &state.log_buffer[start..] {
                    let display = if line.len() > 100 {
                        format!("{}...", &line[..97])
                    } else {
                        line.clone()
                    };
                    logs_menu.addItem(&styled_item(&display, &log_font, None, mtm));
                }
            }
            logs_item.setSubmenu(Some(&logs_menu));
            menu.addItem(&logs_item);

            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&action_item("Quit", sel!(quit:), &this, mtm));

            // Logo banner — inserted at position 0 after all items are added
            // so we can read the menu's computed width and size the logo to match.
            let logo_bytes = include_bytes!("../logo.png");
            let data = NSData::from_vec(logo_bytes.to_vec());
            if let Some(logo_img) = NSImage::initWithData(NSImage::alloc(), &data) {
                let original_w = logo_img.size().width;
                let original_h = logo_img.size().height;

                let menu_w = menu.size().width;
                let target_w = menu_w - 18.0;
                let target_h = target_w * original_h / original_w;
                logo_img.setSize(NSSize::new(target_w, target_h));

                let image_view = NSImageView::imageViewWithImage(&logo_img, mtm);
                let padding = 9.0; // match standard NSMenuItem left padding
                image_view.setFrame(objc2_foundation::NSRect::new(
                    NSPoint::new(padding, 0.0),
                    NSSize::new(target_w, target_h),
                ));
                let container_w = target_w + padding * 2.0;
                let container = {
                    let v = NSView::initWithFrame(
                        mtm.alloc(),
                        objc2_foundation::NSRect::new(
                            NSPoint::new(0.0, 0.0),
                            NSSize::new(container_w, target_h),
                        ),
                    );
                    v.addSubview(&image_view);
                    v
                };
                let logo_item = NSMenuItem::new(mtm);
                logo_item.setView(Some(&container));
                menu.insertItem_atIndex(&logo_item, 0);
                menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), 1);
            }
        }
    }
}

fn styled_item(
    text: &str,
    font: &NSFont,
    color: Option<&NSColor>,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);

    let (keys, actual_color) = unsafe {
        (
            [NSFontAttributeName, NSForegroundColorAttributeName],
            color
                .map(|c| c.retain())
                .unwrap_or_else(NSColor::labelColor),
        )
    };
    let vals: [Retained<AnyObject>; 2] = [
        Retained::into_super(font.retain()).into(),
        Retained::into_super(actual_color).into(),
    ];
    let attrs = NSDictionary::from_retained_objects(&keys, &vals);
    let ns_str = NSString::from_str(text);
    let attr_str = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &ns_str,
            Some(&attrs),
        )
    };
    item.setAttributedTitle(Some(&attr_str));
    item.setEnabled(true);
    item
}

fn gradient_bar_item(
    label: &str,
    utilization: f64,
    width: usize,
    font: &NSFont,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    let filled = ((utilization * width as f64) as usize).min(width);

    unsafe {
        // Label portion in default color
        let label_keys = [NSFontAttributeName, NSForegroundColorAttributeName];
        let label_vals: [Retained<AnyObject>; 2] = [
            Retained::into_super(font.retain()).into(),
            Retained::into_super(NSColor::labelColor()).into(),
        ];
        let label_attrs = NSDictionary::from_retained_objects(&label_keys, &label_vals);
        let result = NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(label),
            Some(&label_attrs),
        );

        // Each bar segment colored by its position
        let filled_char = "\u{25b0}";
        let empty_char = "\u{25b1}";
        for i in 0..width {
            let position = (i as f64 + 0.5) / width as f64;
            let (ch, color) = if i < filled {
                (filled_char, gauge::position_color(position))
            } else {
                (empty_char, gauge::position_color_muted(position))
            };
            let seg_keys = [NSFontAttributeName, NSForegroundColorAttributeName];
            let seg_vals: [Retained<AnyObject>; 2] = [
                Retained::into_super(font.retain()).into(),
                Retained::into_super(color).into(),
            ];
            let seg_attrs = NSDictionary::from_retained_objects(&seg_keys, &seg_vals);
            let seg = NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(ch),
                Some(&seg_attrs),
            );
            result.appendAttributedString(&seg);
        }

        item.setAttributedTitle(Some(&result));
    }
    item.setEnabled(true);
    item
}

fn action_item(
    title: &str,
    action: Sel,
    target: &NSObject,
    mtm: MainThreadMarker,
) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    unsafe {
        item.setAction(Some(action));
        item.setTarget(Some(target));
    }
    item
}

fn save_preferences(prefs: &Preferences) {
    let defaults = NSUserDefaults::standardUserDefaults();
    defaults.setDouble_forKey(prefs.poll_interval, &NSString::from_str("poll_interval"));
    defaults.setDouble_forKey(
        prefs.alert_threshold,
        &NSString::from_str("alert_threshold"),
    );
    defaults.setDouble_forKey(
        prefs.alert_threshold_7d,
        &NSString::from_str("alert_threshold_7d"),
    );
    defaults.setBool_forKey(
        prefs.polling_enabled,
        &NSString::from_str("polling_enabled"),
    );
    defaults.setBool_forKey(
        prefs.show_both_windows,
        &NSString::from_str("show_both_windows"),
    );
}

fn load_preferences() -> Preferences {
    let defaults = NSUserDefaults::standardUserDefaults();
    let interval = defaults.doubleForKey(&NSString::from_str("poll_interval"));
    let threshold = defaults.doubleForKey(&NSString::from_str("alert_threshold"));
    let threshold_7d = defaults.doubleForKey(&NSString::from_str("alert_threshold_7d"));

    // doubleForKey returns 0.0 if not set — use defaults in that case
    let interval = if interval > 0.0 {
        interval
    } else {
        POLL_INTERVAL_DEFAULT
    };
    let threshold = if threshold > 0.0 {
        threshold
    } else {
        ALERT_THRESHOLD_DEFAULT
    };
    let threshold_7d = if threshold_7d > 0.0 {
        threshold_7d
    } else {
        ALERT_THRESHOLD_7D_DEFAULT
    };

    // boolForKey returns false if not set — default to true (polling on)
    let has_polling_key = defaults
        .objectForKey(&NSString::from_str("polling_enabled"))
        .is_some();
    let polling = if has_polling_key {
        defaults.boolForKey(&NSString::from_str("polling_enabled"))
    } else {
        true
    };

    let has_show_both_key = defaults
        .objectForKey(&NSString::from_str("show_both_windows"))
        .is_some();
    let show_both_windows = if has_show_both_key {
        defaults.boolForKey(&NSString::from_str("show_both_windows"))
    } else {
        SHOW_BOTH_WINDOWS_DEFAULT
    };

    Preferences {
        poll_interval: interval,
        alert_threshold: threshold,
        alert_threshold_7d: threshold_7d,
        polling_enabled: polling,
        show_both_windows,
    }
}

fn timestamp() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Identity fingerprint for a keychain credential entry. Changes whenever
/// the user logs in again (refreshToken rotates per login) or switches accounts.
fn creds_fingerprint(creds: &serde_json::Value) -> String {
    let refresh = creds
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let expires = creds.get("expiresAt").and_then(|v| v.as_u64()).unwrap_or(0);
    format!("{refresh}|{expires}")
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("Must run on main thread");

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let delegate = AppDelegate::new(mtm);
    // Keep delegate alive and set as app delegate via runtime
    let delegate_ptr: *const AppDelegate = &*delegate;
    unsafe {
        let _: () = msg_send![&*app, setDelegate: delegate_ptr];
    }

    // Keep delegate retained for lifetime of app
    std::mem::forget(delegate);

    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &'static str, utilization: f64) -> UsageWindow {
        UsageWindow {
            label,
            utilization,
            resets_at: None,
        }
    }

    #[test]
    fn test_alert_decision_below_threshold_not_fired_is_none() {
        assert_eq!(alert_decision(0.50, 0.80, false), AlertDecision::None);
    }

    #[test]
    fn test_alert_decision_below_threshold_fired_resets() {
        assert_eq!(alert_decision(0.50, 0.80, true), AlertDecision::Reset);
    }

    #[test]
    fn test_alert_decision_at_threshold_not_fired_fires() {
        assert_eq!(alert_decision(0.80, 0.80, false), AlertDecision::Fire);
    }

    #[test]
    fn test_alert_decision_at_threshold_already_fired_is_none() {
        assert_eq!(alert_decision(0.80, 0.80, true), AlertDecision::None);
    }

    #[test]
    fn test_alert_decision_above_threshold_not_fired_fires() {
        assert_eq!(alert_decision(0.95, 0.80, false), AlertDecision::Fire);
    }

    #[test]
    fn test_alert_decision_above_threshold_already_fired_is_none() {
        assert_eq!(alert_decision(0.95, 0.80, true), AlertDecision::None);
    }

    #[test]
    fn test_alert_decision_disabled_threshold_is_always_none() {
        // threshold > 1.0 means alerts are off, regardless of util or fired state,
        // including at 100% utilization.
        let cases = [
            (0.0, false),
            (0.0, true),
            (0.50, false),
            (0.50, true),
            (1.0, false),
            (1.0, true),
        ];
        for (util, already_fired) in cases {
            assert_eq!(
                alert_decision(util, 1.01, already_fired),
                AlertDecision::None,
                "util={util} already_fired={already_fired}"
            );
        }
    }

    #[test]
    fn test_find_window_hit_returns_matching_window() {
        let windows = vec![window("5h", 0.42), window("7d", 0.77)];
        let found = find_window(&windows, "7d").expect("expected a match");
        assert_eq!(found.label, "7d");
        assert_eq!(found.utilization, 0.77);
    }

    #[test]
    fn test_find_window_miss_returns_none() {
        let windows = vec![window("5h", 0.42)];
        assert!(find_window(&windows, "7d").is_none());
    }

    #[test]
    fn test_find_window_empty_slice_returns_none() {
        let windows: Vec<UsageWindow> = Vec::new();
        assert!(find_window(&windows, "5h").is_none());
    }

    #[test]
    fn test_primary_is_seven_day_when_primary_fell_back_to_7d() {
        assert!(primary_is_seven_day(&Some(window("7d", 0.85))));
    }

    #[test]
    fn test_primary_is_seven_day_false_for_5h_primary() {
        assert!(!primary_is_seven_day(&Some(window("5h", 0.05))));
    }

    #[test]
    fn test_primary_is_seven_day_false_when_no_primary() {
        assert!(!primary_is_seven_day(&None));
    }

    #[test]
    fn test_alert_checks_routes_5h_primary_and_7d_separately() {
        let windows = vec![window("5h", 0.05), window("7d", 0.85)];
        let checks = alert_checks(&Some(window("5h", 0.05)), &windows);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].0.label, "5h");
        assert_eq!(checks[0].1, AlertSlot::FiveHour);
        assert_eq!(checks[1].0.label, "7d");
        assert_eq!(checks[1].1, AlertSlot::SevenDay);
    }

    // When the API omits the 5h window the primary falls back to 7d. That window must be
    // governed by the 7d threshold, and must be checked exactly once.
    #[test]
    fn test_alert_checks_primary_fallen_back_to_7d_uses_7d_slot_once() {
        let windows = vec![window("7d", 0.85), window("Opus", 0.10)];
        let checks = alert_checks(&Some(window("7d", 0.85)), &windows);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].0.label, "7d");
        assert_eq!(checks[0].1, AlertSlot::SevenDay);
    }

    #[test]
    fn test_alert_checks_primary_fallen_back_to_other_window_still_checks_7d() {
        let windows = vec![window("Opus", 0.10), window("7d", 0.85)];
        let checks = alert_checks(&Some(window("Opus", 0.10)), &windows);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].1, AlertSlot::FiveHour);
        assert_eq!(checks[1].0.label, "7d");
        assert_eq!(checks[1].1, AlertSlot::SevenDay);
    }

    #[test]
    fn test_alert_checks_no_primary_still_checks_7d() {
        let windows = vec![window("7d", 0.85)];
        let checks = alert_checks(&None, &windows);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].1, AlertSlot::SevenDay);
    }

    #[test]
    fn test_alert_checks_empty_windows_no_primary_yields_nothing() {
        let checks = alert_checks(&None, &[]);
        assert!(checks.is_empty());
    }
}
