//! The glow windows themselves: a pool of full-screen, click-through,
//! always-on-top surfaces, one per display.
//!
//! Two invariants drive the whole design:
//!
//! 1. **Windows are pooled and never destroyed.** Upstream registers an
//!    app-wide `CloseRequested` handler that calls `prevent_close()`, hides the
//!    window, and on macOS switches the app to `ActivationPolicy::Accessory` —
//!    so closing a glow window would not destroy it *and* would make the Dock
//!    icon vanish. The pool only ever grows; surplus surfaces are hidden.
//! 2. **Every geometry call runs on the main thread.** Monitor lookups hit
//!    GDK/Xlib on Linux, which hard-crashes when called off the GTK main thread
//!    (upstream issue #227), and AppKit is main-thread-only.
//!    `run_on_main_thread` executes inline when already on it, so this costs
//!    nothing on the hot path.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use log::{debug, warn};
use tauri::{AppHandle, Manager};

#[cfg(target_os = "macos")]
use super::macos;

#[cfg(not(target_os = "macos"))]
use tauri::{WebviewUrl, WebviewWindowBuilder};

/// Served from `public/`, which Vite copies to the dist root untransformed, so
/// no Vite entry point (and therefore no `vite.config.ts` edit) is needed.
const GLOW_URL: &str = "prominent-glow.html";

/// Must outlast `--glow-out` in the page, or the window is yanked mid-fade.
const FADE_OUT_MS: u64 = 320;

/// Number of surfaces built so far. Grow-only; main thread only.
static POOL_LEN: AtomicUsize = AtomicUsize::new(0);
/// Whether the glow is currently meant to be on screen.
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// Invalidates in-flight fade-out timers. A recording that starts during a
/// fade-out must not have its fresh glow torn down by the previous stop's
/// timer — bumping this makes the stale timer a no-op.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn label(index: usize) -> String {
    crate::addons::window_label("glow", index)
}

/// How many surfaces the current display layout wants.
///
/// Clamped to at least one: on Wayland `available_monitors` can legitimately
/// return nothing, and a layer-shell surface does not need monitor geometry to
/// cover its output.
fn monitor_count(app: &AppHandle) -> usize {
    app.available_monitors()
        .map(|monitors| monitors.len())
        .unwrap_or(0)
        .max(1)
}

/// Grows the pool to cover every current display. Safe to call repeatedly.
pub fn ensure_pool(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || ensure_pool_on_main(&handle));
}

fn ensure_pool_on_main(app: &AppHandle) {
    let want = monitor_count(app);
    let have = POOL_LEN.load(Ordering::Relaxed);
    if want <= have {
        return;
    }

    for index in have..want {
        let label = label(index);
        if let Err(error) = create_surface(app, &label) {
            warn!("prominent-glow: failed to create surface {label}: {error}");
            // Stop at the first failure rather than looping on a broken
            // display; the next show() retries.
            POOL_LEN.store(index, Ordering::Relaxed);
            return;
        }
        configure_surface(app, &label, index);
        // Place now so the first show does not have to wait on a deferred
        // resize landing.
        place(app, &label, index);
    }

    POOL_LEN.store(want, Ordering::Relaxed);
    debug!("prominent-glow: pool grown to {want} surface(s)");
}

/// Shows the glow on every display at the given intensity.
pub fn show(app: &AppHandle, intensity: f64) {
    VISIBLE.store(true, Ordering::Relaxed);
    // Cancel any fade-out still counting down from the previous recording.
    GENERATION.fetch_add(1, Ordering::Relaxed);

    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        ensure_pool_on_main(&handle);

        let count = monitor_count(&handle);
        let pool = POOL_LEN.load(Ordering::Relaxed);

        for index in 0..count.min(pool) {
            let label = label(index);
            // Re-placing on every show is what makes display hot-plug work
            // without subscribing to any platform display-change notification.
            place(&handle, &label, index);
            apply_intensity(&handle, &label, intensity);
            show_surface(&handle, &label, index);
        }

        // Displays that went away since the pool was built. Without this, a
        // 3-monitor pool on a now-1-monitor desk would stack three glows onto
        // the surviving display at triple strength.
        for index in count..pool {
            hide_surface(&handle, &label(index));
        }
    });
}

/// Fades the glow out, then hides the surfaces once the fade has played.
pub fn hide(app: &AppHandle) {
    if !VISIBLE.swap(false, Ordering::Relaxed) {
        return;
    }

    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;

    for index in 0..POOL_LEN.load(Ordering::Relaxed) {
        eval(app, &label(index), "window.__glow && window.__glow.hide()");
    }

    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(FADE_OUT_MS));
        let inner = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            // Checked *inside* the posted task, not before posting it: a
            // recording that starts in between bumps the generation, and
            // main-thread tasks run FIFO, so testing here is decisive. Testing
            // before the post would let a fresh glow be torn down by the
            // previous stop's timer.
            if GENERATION.load(Ordering::Relaxed) != generation {
                return;
            }
            for index in 0..POOL_LEN.load(Ordering::Relaxed) {
                hide_surface(&inner, &label(index));
            }
        });
    });
}

/// Live intensity change while the glow is on screen (slider drag).
pub fn set_intensity(app: &AppHandle, intensity: f64) {
    if !VISIBLE.load(Ordering::Relaxed) {
        return;
    }
    for index in 0..POOL_LEN.load(Ordering::Relaxed) {
        apply_intensity(app, &label(index), intensity);
    }
}

fn apply_intensity(app: &AppHandle, label: &str, intensity: f64) {
    eval(
        app,
        label,
        &format!("window.__glow && window.__glow.set({{'glow-alpha':{intensity}}})"),
    );
}

/// `eval` is not ACL-gated, which is what lets the glow page stay outside every
/// capability file and hold zero Tauri JS API access of its own.
fn eval(app: &AppHandle, label: &str, script: &str) {
    if let Some(window) = app.get_webview_window(label) {
        if let Err(error) = window.eval(script) {
            debug!("prominent-glow: eval on {label} failed: {error}");
        }
    }
}

// ---------------------------------------------------------------- creation --

#[cfg(target_os = "macos")]
fn create_surface(app: &AppHandle, label: &str) -> Result<(), String> {
    macos::create_panel(app, label, GLOW_URL)
}

#[cfg(not(target_os = "macos"))]
fn create_surface(app: &AppHandle, label: &str) -> Result<(), String> {
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(GLOW_URL.into()))
        .title("Recording indicator")
        .decorations(false)
        .shadow(false)
        .transparent(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        // `focusable(false)` gives WS_EX_NOACTIVATE on Windows; `focused(false)`
        // is what makes the first show() use SW_SHOWNOACTIVATE. Both are needed
        // or the glow steals activation the first time it appears.
        .focusable(false)
        .focused(false)
        .inner_size(1.0, 1.0)
        .visible(false);

    if let Some(data_dir) = crate::portable::data_dir() {
        // Share the webview data dir with the rest of the app so these are
        // extra renderers, not extra browser processes.
        builder = builder.data_directory(data_dir.join("webview"));
    }

    builder.build().map(|_| ()).map_err(|e| e.to_string())
}

// ----------------------------------------------------------- configuration --

/// One-time per-surface setup that must happen after build but before the first
/// show.
#[cfg(target_os = "macos")]
fn configure_surface(_app: &AppHandle, _label: &str, _index: usize) {
    // Click-through, level, collection behaviour and transparency are all set
    // synchronously by the panel builder.
}

#[cfg(target_os = "windows")]
fn configure_surface(app: &AppHandle, label: &str, _index: usize) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    // tao holds this as a window flag and re-derives WS_EX_TRANSPARENT |
    // WS_EX_LAYERED from it on every style change, so it survives the ex-style
    // rewrite that each show()/hide() performs. Setting the bit by hand with
    // SetWindowLongPtrW would be wiped on the first show.
    if let Err(error) = window.set_ignore_cursor_events(true) {
        warn!("prominent-glow: {label} could not be made click-through: {error}");
    }
}

#[cfg(target_os = "linux")]
fn configure_surface(app: &AppHandle, label: &str, index: usize) {
    // `Display::default()` and `Display::monitor()` are inherent in gdk 0.18 —
    // there is no `DisplayExt` trait to import.
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let gtk_window = match window.gtk_window() {
        Ok(w) => w,
        Err(error) => {
            warn!("prominent-glow: {label} has no GTK window: {error}");
            return;
        }
    };

    // Click-through on both backends: X11 maps an empty input shape to
    // XShapeCombineRectangles(ShapeInput, 0 rects), Wayland to
    // wl_surface_set_input_region(empty).
    //
    // Deliberately not `set_ignore_cursor_events(true)`: tao's Linux
    // implementation installs a 1x1 region at (0,0) rather than an empty one,
    // which would leave a live input pixel sitting exactly on the GNOME/KDE
    // top-left hot corner. Setting it at the widget level means GTK replays it
    // on realize, so it is safe on a still-hidden window.
    gtk_window.input_shape_combine_region(Some(&gtk::cairo::Region::create()));

    let monitor = gtk::gdk::Display::default().and_then(|display| display.monitor(index as i32));

    // Same escape hatch the pill overlay honours (see AGENTS.md).
    let layer_shell_disabled = std::env::var("HANDY_NO_GTK_LAYER_SHELL")
        .map(|v| {
            !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false);

    if !layer_shell_disabled && gtk_layer_shell::is_supported() {
        gtk_window.init_layer_shell();
        gtk_window.set_layer(Layer::Overlay);
        gtk_window.set_keyboard_mode(KeyboardMode::None);
        // -1 means "do not shrink me out of another surface's reserved strip";
        // 0 (what the pill overlay uses) would leave an unglowing band along
        // any panel or dock.
        gtk_window.set_exclusive_zone(-1);

        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            gtk_window.set_anchor(edge, true);
            gtk_window.set_layer_shell_margin(edge, 0);
        }
        // With opposite edges anchored, gtk-layer-shell requests size (0,0),
        // which the protocol defines as "compositor, fill the output". So no
        // set_size_request/resize dance here, unlike the compact overlay.
        gtk_window.set_size_request(-1, -1);

        // The default is "compositor decides", which lands every surface on one
        // output. Pin each to its own monitor.
        if let Some(monitor) = &monitor {
            gtk_window.set_monitor(monitor);
        }
        return;
    }

    // X11 / no-layer-shell fallback. The Notification type hint is what keeps
    // the surface above normal windows under EWMH window managers; plain
    // always_on_top is only _NET_WM_STATE_ABOVE, which many WMs let ordinary
    // windows override. It must be set before the window is mapped.
    gtk_window.set_type_hint(gtk::gdk::WindowTypeHint::Notification);
    gtk_window.set_keep_above(true);
    gtk_window.set_skip_taskbar_hint(true);
    gtk_window.set_skip_pager_hint(true);
    gtk_window.set_accept_focus(false);
    gtk_window.stick();
}

// --------------------------------------------------------------- placement --

/// Resizes and moves a surface to cover its display.
#[cfg(target_os = "macos")]
fn place(app: &AppHandle, label: &str, index: usize) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let Some(monitor) = app
        .available_monitors()
        .ok()
        .and_then(|monitors| monitors.into_iter().nth(index))
    else {
        return;
    };

    // Logical, not physical: tao converts a PhysicalPosition using the scale
    // factor of the monitor the window is *currently* on, which mislands every
    // cross-monitor move. Same reasoning (and same fix) as
    // `overlay::calculate_overlay_position`.
    let scale = monitor.scale_factor();
    let position = tauri::LogicalPosition {
        x: monitor.position().x as f64 / scale,
        y: monitor.position().y as f64 / scale,
    };
    let size = tauri::LogicalSize {
        width: monitor.size().width as f64 / scale,
        height: monitor.size().height as f64 / scale,
    };

    let _ = window.set_size(tauri::Size::Logical(size));
    let _ = window.set_position(tauri::Position::Logical(position));
}

/// Windows places natively: `Monitor::position`/`size` are `rcMonitor`, i.e.
/// already virtual-screen physical pixels, so the full-monitor rect goes
/// straight into `SetWindowPos` with no scale factor anywhere — and no chance
/// of tao's current-DPI logical conversion mislanding it.
#[cfg(target_os = "windows")]
fn place(app: &AppHandle, label: &str, index: usize) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let Some(monitor) = app
        .available_monitors()
        .ok()
        .and_then(|monitors| monitors.into_iter().nth(index))
    else {
        return;
    };
    windows_set_bounds(&window, *monitor.position(), *monitor.size());
}

#[cfg(target_os = "windows")]
fn windows_set_bounds(
    window: &tauri::webview::WebviewWindow,
    position: tauri::PhysicalPosition<i32>,
    size: tauri::PhysicalSize<u32>,
) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOOWNERZORDER,
    };

    let hwnd = match window.hwnd() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            warn!("prominent-glow: no window handle: {error}");
            return;
        }
    };

    unsafe {
        if let Err(error) = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            position.x,
            position.y,
            size.width as i32,
            size.height as i32,
            SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        ) {
            warn!("prominent-glow: SetWindowPos failed: {error}");
        }
    }
}

/// Layer-shell surfaces derive their geometry from their anchors and are
/// already pinned to a monitor, so there is nothing to place. The X11 fallback
/// is moved after show(), where a move actually sticks.
#[cfg(target_os = "linux")]
fn place(_app: &AppHandle, _label: &str, _index: usize) {}

// ------------------------------------------------------------- show / hide --

#[cfg(target_os = "macos")]
fn show_surface(app: &AppHandle, label: &str, _index: usize) {
    macos::show_panel(app, label);
    eval(app, label, "window.__glow && window.__glow.show()");
}

#[cfg(target_os = "windows")]
fn show_surface(app: &AppHandle, label: &str, index: usize) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let _ = window.show();
    // show() runs tao's style diff and, across a DPI boundary, a WM_DPICHANGED
    // reflow that rescales us to preserve *logical* size — clobbering the
    // physical full-monitor rect. Re-assert, exactly as the pill overlay does.
    place(app, label, index);
    eval(app, label, "window.__glow && window.__glow.show()");
}

#[cfg(target_os = "linux")]
fn show_surface(app: &AppHandle, label: &str, index: usize) {
    use gtk::gdk::prelude::MonitorExt;
    use gtk::prelude::GtkWindowExt;
    use gtk_layer_shell::LayerShell;

    let Some(window) = app.get_webview_window(label) else {
        return;
    };

    let gtk_window = window.gtk_window().ok();
    let monitor = gtk::gdk::Display::default().and_then(|display| display.monitor(index as i32));

    // Re-pin before showing, while the surface is still unmapped.
    //
    // Calling set_monitor at all opts the surface out of gtk-layer-shell's own
    // monitor-added/removed remap ("if the surface has a monitor set, it is in
    // charge of responding to monitor changes"), so we have to take that job
    // over. Without this, unplugging a display leaves surface 0 pinned to the
    // GdkMonitor that just went away and the remaining screen gets no glow.
    // The C side early-returns when the pointer is unchanged, so the
    // steady-state cost is nil.
    if let (Some(gtk_window), Some(monitor)) = (gtk_window.as_ref(), monitor.as_ref()) {
        if gtk_window.is_layer_window() {
            gtk_window.set_monitor(monitor);
        }
    }

    let _ = window.show();

    // X11 fallback only: a move issued before the window is mapped is advisory,
    // so it has to be repeated once the window is up.
    if let (Some(gtk_window), Some(monitor)) = (gtk_window.as_ref(), monitor.as_ref()) {
        if !gtk_window.is_layer_window() {
            // GDK geometry is logical, and gtk_window_move/resize want logical
            // too, so these numbers are used as-is (Tauri would report them
            // already multiplied by the scale factor).
            let geometry = monitor.geometry();
            gtk_window.resize(geometry.width(), geometry.height());
            gtk_window.move_(geometry.x(), geometry.y());
        }
    }

    eval(app, label, "window.__glow && window.__glow.show()");
}

#[cfg(target_os = "macos")]
fn hide_surface(app: &AppHandle, label: &str) {
    macos::hide_panel(app, label);
}

#[cfg(not(target_os = "macos"))]
fn hide_surface(app: &AppHandle, label: &str) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.hide();
    }
}
