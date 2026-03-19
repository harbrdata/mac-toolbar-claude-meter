use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
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
const ALERT_THRESHOLD_DEFAULT: f64 = 0.95;
const ALERT_THRESHOLD_OPTIONS: &[u64] = &[75, 80, 85, 90, 95, 100];

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
    log_buffer: Vec<String>,
    log_write_count: u32,
    alert_threshold: f64,
    alert_fired: bool,
}

impl AppState {
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
    #[ivars = RefCell<AppState>]
    pub struct AppDelegate;

    impl AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.setup();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            launch_agent::cleanup_if_uninstalled();
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            // If the .app bundle was deleted, clean up and quit
            if !std::path::Path::new("/Applications/Claude-o-Meter.app").exists() {
                launch_agent::cleanup_if_uninstalled();
                let state = self.ivars().borrow();
                let app = NSApplication::sharedApplication(state.mtm);
                app.terminate(None);
                return;
            }

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
            let state = self.ivars().borrow();
            save_preferences(state.poll_interval, state.alert_threshold, state.polling_enabled);
            drop(state);
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
        // Load saved preferences
        let (saved_interval, saved_threshold, saved_polling) = load_preferences();

        let this = mtm.alloc::<AppDelegate>();
        let this = this.set_ivars(RefCell::new(AppState {
            mtm,
            status_item: None,
            menu: None,
            poll_interval: saved_interval,
            poll_timer: None,
            polling_enabled: saved_polling,
            last_windows: Vec::new(),
            last_primary: None,
            rate_limited: false,
            rate_limit_resume: None,
            rate_limit_backoff: 0,
            rate_limit_timer: None,
            rate_limit_countdown_timer: None,
            cached_token: None,
            cached_token_expires: None,
            log_buffer: Vec::new(),
            log_write_count: 0,
            alert_threshold: saved_threshold,
            alert_fired: false,
        }));
        unsafe { msg_send![super(this), init] }
    }

    fn mtm(&self) -> MainThreadMarker {
        self.ivars().borrow().mtm
    }

    fn setup(&self) {
        let _ = std::fs::create_dir_all(launch_agent::log_dir());
        launch_agent::rotate_log_if_needed();

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
                2.0,
                &this,
                sel!(tick:),
                None,
                false,
            );

            let state = self.ivars().borrow();
            let saved_interval = state.poll_interval;
            let polling = state.polling_enabled;
            drop(state);

            if polling {
                self.start_timer(saved_interval);
            } else {
                self.set_icon(&gauge::create_paused_icon(ICON_SIZE));
            }
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

    fn set_alert_threshold(&self, threshold: f64) {
        let mut state = self.ivars().borrow_mut();
        state.alert_threshold = threshold;
        state.alert_fired = false;
        let interval = state.poll_interval;
        if threshold > 1.0 {
            state.push_log(format!("{} Usage alert disabled", timestamp()));
        } else {
            state.push_log(format!("{} Alert threshold set to {}%", timestamp(), (threshold * 100.0) as u32));
        }
        drop(state);
        save_preferences(interval, threshold, self.ivars().borrow().polling_enabled);
        self.rebuild_menu();
    }

    fn check_and_fire_alert(&self) {
        let mut state = self.ivars().borrow_mut();
        let threshold = state.alert_threshold;
        if threshold > 1.0 {
            return; // alerts disabled
        }
        let primary = state.last_primary.clone();
        if let Some(primary) = primary {
            let util = primary.utilization;
            if util >= threshold && !state.alert_fired {
                state.alert_fired = true;
                let pct = (util * 100.0) as u32;
                state.push_log(format!("{} Alert: {} usage at {}%", timestamp(), primary.label, pct));
                drop(state);
                let body = format!("{} window usage is at {}%", primary.label, pct);
                let _ = std::process::Command::new("osascript")
                    .args(["-e", &format!(
                        "display notification \"{}\" with title \"Claude Meter\" subtitle \"Usage alert\"",
                        body
                    )])
                    .spawn();
            } else if util < threshold && state.alert_fired {
                state.alert_fired = false;
            }
        }
    }

    fn set_interval(&self, seconds: f64) {
        let mut state = self.ivars().borrow_mut();
        state.push_log(format!("{} Poll interval changed to {}s", timestamp(), seconds as u64));
        drop(state);
        self.start_timer(seconds);
        let state = self.ivars().borrow();
        save_preferences(seconds, state.alert_threshold, state.polling_enabled);
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

        // Check if cached token has expired
        let token_expired = match state.cached_token_expires {
            Some(expires) => std::time::Instant::now() >= expires,
            None => state.cached_token.is_none(),
        };

        if token_expired {
            state.cached_token = None;
            state.cached_token_expires = None;

            if let Some(creds) = keychain::read_credentials() {
                if let Some(result) = api::get_access_token(&creds) {
                    state.cached_token = Some(result.access_token);
                    if let Some(expires_in) = result.expires_in_secs {
                        // Expire 60s early to avoid using a nearly-expired token
                        let buffer = expires_in.saturating_sub(60);
                        state.cached_token_expires = Some(
                            std::time::Instant::now() + std::time::Duration::from_secs(buffer)
                        );
                    }
                }
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
                self.check_and_fire_alert();
                self.rebuild_menu();
            }
            FetchResult::RateLimited(retry_after) => {
                self.enter_rate_limit_pause(retry_after);
            }
            FetchResult::AuthError => {
                let mut state = self.ivars().borrow_mut();
                state.cached_token = None;
                state.cached_token_expires = None;
                state.push_log(format!("{} [WARN] Auth error, will retry", timestamp()));
                drop(state);
                self.show_error();
            }
            FetchResult::Error(e) => {
                let mut state = self.ivars().borrow_mut();
                state.cached_token = None;
                state.cached_token_expires = None;
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
                    let reset = api::format_reset_time(w.resets_at.as_deref());

                    let label_text = format!(" {}: {}%  ", w.label, pct);
                    let line = gradient_bar_item(&label_text, w.utilization, 20, &mono, mtm);
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

            // Alert threshold submenu
            let alert_item = NSMenuItem::new(mtm);
            alert_item.setTitle(&NSString::from_str("Alert Threshold"));
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

fn gradient_bar_item(label: &str, utilization: f64, width: usize, font: &NSFont, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
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

fn action_item(title: &str, action: Sel, target: &NSObject, mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    unsafe {
        item.setAction(Some(action));
        item.setTarget(Some(target));
    }
    item
}

fn save_preferences(poll_interval: f64, alert_threshold: f64, polling_enabled: bool) {
    let defaults = NSUserDefaults::standardUserDefaults();
    defaults.setDouble_forKey(poll_interval, &NSString::from_str("poll_interval"));
    defaults.setDouble_forKey(alert_threshold, &NSString::from_str("alert_threshold"));
    defaults.setBool_forKey(polling_enabled, &NSString::from_str("polling_enabled"));
}

fn load_preferences() -> (f64, f64, bool) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let interval = defaults.doubleForKey(&NSString::from_str("poll_interval"));
    let threshold = defaults.doubleForKey(&NSString::from_str("alert_threshold"));

    // doubleForKey returns 0.0 if not set — use defaults in that case
    let interval = if interval > 0.0 { interval } else { POLL_INTERVAL_DEFAULT };
    let threshold = if threshold > 0.0 { threshold } else { ALERT_THRESHOLD_DEFAULT };

    // boolForKey returns false if not set — default to true (polling on)
    let has_key = defaults.objectForKey(&NSString::from_str("polling_enabled")).is_some();
    let polling = if has_key { defaults.boolForKey(&NSString::from_str("polling_enabled")) } else { true };

    (interval, threshold, polling)
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
