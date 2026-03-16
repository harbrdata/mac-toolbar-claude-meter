use std::cell::RefCell;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker, MainThreadOnly, DefinedClass, Message};
use objc2_app_kit::*;
use objc2_foundation::*;

use crate::api::{self, FetchResult, UsageWindow};
use crate::gauge;
use crate::keychain;
use crate::launch_agent;

const ICON_SIZE: f64 = 24.0;
const POLL_INTERVAL_DEFAULT: f64 = 120.0;
const POLL_INTERVAL_OPTIONS: &[u64] = &[60, 120, 300, 600];
const RATE_LIMIT_PAUSE_DEFAULT: u64 = 60;
const RATE_LIMIT_PAUSE_MAX: u64 = 600;
const LOG_CAPACITY: usize = 20;

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
    log_buffer: Vec<String>,
}

impl AppState {
    fn push_log(&mut self, msg: String) {
        if self.log_buffer.len() >= LOG_CAPACITY {
            self.log_buffer.remove(0);
        }
        self.log_buffer.push(msg);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AppDelegate"]
    #[ivars = RefCell<AppState>]
    pub struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.setup();
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            let state = self.ivars().borrow();
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
            let mut state = self.ivars().borrow_mut();
            state.polling_enabled = !state.polling_enabled;

            if state.polling_enabled {
                state.push_log(format!("{} Polling enabled", timestamp()));
                let interval = state.poll_interval;
                drop(state);
                self.start_timer(interval);
                let state = self.ivars().borrow();
                if let Some(ref primary) = state.last_primary {
                    self.set_icon(&gauge::create_gauge_icon(primary.utilization, ICON_SIZE));
                }
            } else {
                state.push_log(format!("{} Polling disabled", timestamp()));
                if let Some(ref timer) = state.poll_timer {
                    timer.invalidate();
                }
                state.poll_timer = None;
                drop(state);
                self.set_icon(&gauge::create_paused_icon(ICON_SIZE));
            }
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
            let mut state = self.ivars().borrow_mut();
            state.push_log(format!("{} Rate-limit pause expired, resuming", timestamp()));
            drop(state);
            self.clear_rate_limit();
            self.fetch_and_update();
        }

        #[unsafe(method(rateLimitCountdown:))]
        fn rate_limit_countdown(&self, _timer: &NSTimer) {
            let state = self.ivars().borrow();
            if state.rate_limited {
                drop(state);
                self.rebuild_menu();
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &AnyObject) {
            let state = self.ivars().borrow();
            let app = NSApplication::sharedApplication(state.mtm);
            app.terminate(None);
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<AppDelegate>();
        let this = this.set_ivars(RefCell::new(AppState {
            mtm,
            status_item: None,
            menu: None,
            poll_interval: POLL_INTERVAL_DEFAULT,
            poll_timer: None,
            polling_enabled: true,
            last_windows: Vec::new(),
            last_primary: None,
            rate_limited: false,
            rate_limit_resume: None,
            rate_limit_backoff: 0,
            rate_limit_timer: None,
            rate_limit_countdown_timer: None,
            cached_token: None,
            log_buffer: Vec::new(),
        }));
        unsafe { msg_send![super(this), init] }
    }

    fn mtm(&self) -> MainThreadMarker {
        self.ivars().borrow().mtm
    }

    fn setup(&self) {
        let mtm = self.mtm();
        unsafe {
            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);

            if let Some(button) = status_item.button(mtm) {
                button.setImage(Some(&gauge::create_gauge_icon(0.0, ICON_SIZE)));
                button.setTitle(&NSString::from_str(""));
            }

            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            status_item.setMenu(Some(&menu));

            {
                let mut state = self.ivars().borrow_mut();
                state.status_item = Some(status_item);
                state.menu = Some(menu);
                state.push_log(format!("{} Claude Meter starting...", timestamp()));
            }

            self.rebuild_menu();

            let this: Retained<NSObject> = Retained::into_super(
                self.retain(),
            );
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.1,
                &this,
                sel!(tick:),
                None,
                false,
            );

            self.start_timer(POLL_INTERVAL_DEFAULT);
        }
    }

    fn start_timer(&self, interval: f64) {
        let mut state = self.ivars().borrow_mut();
        if let Some(ref timer) = state.poll_timer {
            timer.invalidate();
        }
        state.poll_interval = interval;
        unsafe {
            let this: Retained<NSObject> = Retained::into_super(
                self.retain(),
            );
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

    fn set_interval(&self, seconds: f64) {
        let mut state = self.ivars().borrow_mut();
        state.push_log(format!("{} Poll interval changed to {}s", timestamp(), seconds as u64));
        drop(state);
        self.start_timer(seconds);
        self.rebuild_menu();
    }

    fn set_icon(&self, icon: &NSImage) {
        let state = self.ivars().borrow();
        let mtm = state.mtm;
        if let Some(ref si) = state.status_item {
            if let Some(button) = si.button(mtm) {
                button.setImage(Some(icon));
                button.setTitle(&NSString::from_str(""));
            }
        }
    }

    fn fetch_and_update(&self) {
        let mut state = self.ivars().borrow_mut();
        state.push_log(format!("{} Fetching usage data...", timestamp()));

        if state.cached_token.is_none() {
            if let Some(creds) = keychain::read_credentials() {
                state.cached_token = api::get_access_token(&creds);
            } else {
                state.push_log(format!("{} [ERROR] No credentials found", timestamp()));
                drop(state);
                self.show_error();
                return;
            }
        }

        let Some(token) = state.cached_token.clone() else {
            state.push_log(format!("{} [ERROR] No access token", timestamp()));
            drop(state);
            self.show_error();
            return;
        };
        drop(state);

        match api::fetch_usage(&token) {
            FetchResult::Ok(data) => {
                let windows = api::parse_usage(&data);
                let primary = windows.iter().find(|w| w.label == "5h").cloned()
                    .or_else(|| windows.first().cloned());

                let mut state = self.ivars().borrow_mut();
                state.push_log(format!("{} Got {} usage windows", timestamp(), windows.len()));
                state.rate_limit_backoff = 0;
                state.last_windows = windows;
                state.last_primary = primary.clone();
                drop(state);

                if let Some(ref p) = primary {
                    self.set_icon(&gauge::create_gauge_icon(p.utilization, ICON_SIZE));
                } else {
                    self.set_icon(&gauge::create_gauge_icon(0.0, ICON_SIZE));
                }
                self.rebuild_menu();
            }
            FetchResult::RateLimited(retry_after) => {
                self.enter_rate_limit_pause(retry_after);
            }
            FetchResult::AuthError => {
                let mut state = self.ivars().borrow_mut();
                state.cached_token = None;
                state.push_log(format!("{} [WARN] Auth error, will retry", timestamp()));
                drop(state);
                self.show_error();
            }
            FetchResult::Error(e) => {
                let mut state = self.ivars().borrow_mut();
                state.cached_token = None;
                state.push_log(format!("{} [ERROR] {}", timestamp(), e));
                drop(state);
                self.show_error();
            }
        }
    }

    fn show_error(&self) {
        let state = self.ivars().borrow();
        if !state.last_windows.is_empty() {
            let primary = state.last_primary.clone();
            drop(state);
            if let Some(ref p) = primary {
                self.set_icon(&gauge::create_gauge_icon(p.utilization, ICON_SIZE));
            }
            self.rebuild_menu();
        } else {
            drop(state);
            self.set_icon(&gauge::create_error_icon(ICON_SIZE));
            self.rebuild_menu();
        }
    }

    fn enter_rate_limit_pause(&self, retry_after: u64) {
        let mut state = self.ivars().borrow_mut();
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
            timestamp(), pause, backoff
        ));

        if let Some(ref t) = state.rate_limit_timer {
            t.invalidate();
        }
        if let Some(ref t) = state.rate_limit_countdown_timer {
            t.invalidate();
        }

        unsafe {
            let this: Retained<NSObject> = Retained::into_super(
                self.retain(),
            );
            state.rate_limit_timer = Some(
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    pause as f64,
                    &this,
                    sel!(rateLimitResume:),
                    None,
                    false,
                ),
            );
            let this2: Retained<NSObject> = Retained::into_super(
                self.retain(),
            );
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

        self.set_icon(&gauge::create_paused_icon(ICON_SIZE));
        self.rebuild_menu();
    }

    fn clear_rate_limit(&self) {
        let mut state = self.ivars().borrow_mut();
        state.rate_limited = false;
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
        let state = self.ivars().borrow();
        let Some(ref menu) = state.menu else { return };
        let mtm = state.mtm;

        {
            menu.removeAllItems();
            let mono = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 12.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(12.0));
            let mono_small = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 11.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(11.0));

            // Rate-limit banner
            if state.rate_limited {
                if let Some(ref resume_at) = state.rate_limit_resume {
                    let remaining = resume_at.saturating_duration_since(Instant::now()).as_secs();
                    let mins = remaining / 60;
                    let secs = remaining % 60;
                    let banner = styled_item(
                        &format!("\u{26a0}\u{fe0f}  Rate limited \u{2014} polling paused ({mins}m {secs}s)"),
                        &mono,
                        Some(&NSColor::systemOrangeColor()),
                        mtm,
                    );
                    menu.addItem(&banner);
                    menu.addItem(&NSMenuItem::separatorItem(mtm));
                }
            }

            // Usage windows
            if state.last_windows.is_empty() {
                menu.addItem(&styled_item("Loading...", &mono, Some(&NSColor::secondaryLabelColor()), mtm));
            } else {
                for w in &state.last_windows {
                    let pct = (w.utilization * 100.0) as i32;
                    let bar = gauge::bar_chart(w.utilization, 20);
                    let reset = api::format_reset_time(w.resets_at.as_deref());

                    let line = styled_item(
                        &format!(" {}: {}%  {}", w.label, pct, bar),
                        &mono,
                        None,
                        mtm,
                    );
                    line.setImage(Some(&gauge::create_gauge_icon(w.utilization, 16.0)));
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
            let this: Retained<NSObject> = Retained::into_super(
                self.retain(),
            );

            menu.addItem(&action_item("Refresh Now", sel!(refresh:), &this, mtm));

            let polling_label = if state.polling_enabled { "Polling: On" } else { "Polling: Off" };
            menu.addItem(&action_item(polling_label, sel!(togglePolling:), &this, mtm));

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

            // Login item toggle
            let login_label = if launch_agent::is_enabled() {
                "Start at Login: On"
            } else {
                "Start at Login: Off"
            };
            menu.addItem(&action_item(login_label, sel!(toggleLoginItem:), &this, mtm));

            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Logs submenu
            let logs_item = NSMenuItem::new(mtm);
            logs_item.setTitle(&NSString::from_str("Recent Logs"));
            let logs_menu = NSMenu::new(mtm);
            logs_menu.setAutoenablesItems(false);
            let log_font = NSFont::fontWithName_size(&NSString::from_str("Menlo"), 10.0)
                .unwrap_or_else(|| NSFont::systemFontOfSize(10.0));
            if state.log_buffer.is_empty() {
                logs_menu.addItem(&styled_item("(no logs yet)", &log_font, Some(&NSColor::secondaryLabelColor()), mtm));
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
        }
    }
}

fn styled_item(text: &str, font: &NSFont, color: Option<&NSColor>, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);

    let (keys, actual_color) = unsafe {
        (
            [NSFontAttributeName, NSForegroundColorAttributeName],
            color.map(|c| c.retain()).unwrap_or_else(|| NSColor::labelColor()),
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

fn action_item(title: &str, action: Sel, target: &NSObject, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    unsafe {
        item.setAction(Some(action));
        item.setTarget(Some(target));
    }
    item
}

fn timestamp() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
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
