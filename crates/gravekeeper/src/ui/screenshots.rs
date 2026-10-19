//! Drives the real UI headlessly with egui_kittest: interaction tests, and (on demand)
//! screenshots of every page in both themes.
//!
//!     GRAVEKEEPER_SCREENSHOTS=target/screens cargo test -p gravekeeper screenshots -- --ignored

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use gravekeeper_core::CoreHandle;
use gravekeeper_core::paths::Paths;

use super::{GravekeeperApp, Tab, theme};
use crate::shell::CloseBehavior;

const CONFIG: &str = r#"
schema_version = 2
[monitoring]
interval = 1

[[app]]
id = "web"
name = "Web server"
program = "/bin/sleep"
args = ["600"]
autostart = true
startup_grace = 0

[[app]]
id = "worker"
name = "Queue worker"
program = "/bin/false"
autostart = true
[app.restart]
max_restarts = 0

[[app]]
id = "backup"
name = "Backup agent"
program = "/bin/sleep"
args = ["600"]

[[schedule]]
id = "nightly"
name = "Nightly restart"
when = { kind = "weekly", days = ["mon", "thu"], time = "03:00" }
action = { kind = "restart", app = "web" }

[[schedule]]
id = "cleanup"
name = "Clean temporary files"
when = { kind = "interval", every = 3600 }
action = { kind = "run", program = "/bin/echo", args = ["cleaned"] }

[[webhook]]
id = "ops"
name = "Ops channel"
url = "https://hooks.example.com/services/T000/B000/secret"
format = "slack"
"#;

/// An engine with sample applications. Leaked: the UI borrows it for `'static`.
fn sample_core() -> &'static CoreHandle {
    let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
    std::fs::write(dir.path().join("config.toml"), CONFIG).unwrap();
    let core = CoreHandle::start(Paths::in_dir(dir.path()), Arc::new(|| {})).unwrap();
    let core: &'static CoreHandle = Box::leak(Box::new(core));
    // Let autostarts settle and the first two resource samples arrive.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let s = core.snapshot();
        if s.metrics.cpu_percent.is_some()
            && s.apps
                .iter()
                .all(|a| a.state != gravekeeper_core::api::AppState::Waiting)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    core
}

fn open_ui(
    core: &'static CoreHandle,
    tab: Tab,
    theme: egui::Theme,
    render: bool,
) -> Harness<'static, GravekeeperApp<'static>> {
    let mut builder = Harness::builder()
        .with_size(egui::vec2(1200.0, 780.0))
        .with_theme(theme)
        .with_max_steps(20)
        .with_context_setup(theme::install);
    if render {
        builder = builder.wgpu();
    }
    let mut harness = builder.build_eframe(move |_cc| {
        let mut app = GravekeeperApp::detached(core, CloseBehavior::Destroy, Instant::now());
        app.set_tab(tab);
        app
    });
    harness.run_steps(4);
    harness
}

#[test]
fn adding_an_application_validates_the_form() {
    let core = sample_core();
    let mut harness = open_ui(core, Tab::Apps, egui::Theme::Dark, false);

    harness.get_by_label_contains("Add application").click();
    harness.run_steps(2);
    assert!(harness.query_by_label_contains("Executable").is_some());
    assert!(harness.query_by_label_contains("Browse").is_some());

    // Saving an empty form shows what is missing instead of saving.
    harness.get_by_label_contains("Save").click();
    harness.run_steps(2);
    assert!(
        harness
            .query_by_label_contains("name must not be empty")
            .is_some()
    );
    assert!(
        harness
            .query_by_label_contains("program must not be empty")
            .is_some()
    );

    harness.get_by_label("Cancel").click();
    harness.run_steps(2);
    assert!(harness.query_by_label_contains("Executable").is_none());
}

#[test]
fn every_page_renders_in_the_sidebar_navigation() {
    let core = sample_core();
    let mut harness = open_ui(core, Tab::Dashboard, egui::Theme::Light, false);
    for page in [
        "Applications",
        "Schedules",
        "Notifications",
        "Events",
        "Settings",
        "Dashboard",
    ] {
        harness.get_by_role_and_label(egui::Role::Tab, page).click();
        harness.run_steps(2);
    }
    // The schedule editor is reachable too.
    harness
        .get_by_role_and_label(egui::Role::Tab, "Schedules")
        .click();
    harness.run_steps(2);
    harness.get_by_label_contains("Add schedule").click();
    harness.run_steps(2);
    assert!(harness.query_by_label_contains("Missed runs").is_some());
}

/// Writes PNGs of every page in both themes to `$GRAVEKEEPER_SCREENSHOTS` (needs a GPU or a
/// software Vulkan/GL driver).
#[test]
#[ignore = "renders with wgpu; run explicitly to produce screenshots"]
fn screenshots() {
    let Some(out) = std::env::var_os("GRAVEKEEPER_SCREENSHOTS") else {
        eprintln!("set GRAVEKEEPER_SCREENSHOTS to an output directory");
        return;
    };
    let out = Path::new(&out);
    std::fs::create_dir_all(out).unwrap();
    let core = sample_core();
    let pages = [
        (Tab::Dashboard, "dashboard"),
        (Tab::Apps, "applications"),
        (Tab::Schedules, "schedules"),
        (Tab::Notifications, "notifications"),
        (Tab::Events, "events"),
        (Tab::Settings, "settings"),
    ];
    for (theme, theme_name) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        for (tab, name) in pages {
            let mut harness = open_ui(core, tab, theme, true);
            save(&mut harness, &out.join(format!("{name}-{theme_name}.png")));
        }
        let mut harness = open_ui(core, Tab::Apps, theme, true);
        harness.get_by_label_contains("Add application").click();
        harness.run_steps(3);
        save(
            &mut harness,
            &out.join(format!("app-editor-{theme_name}.png")),
        );

        let mut harness = open_ui(core, Tab::Schedules, theme, true);
        harness.get_by_label_contains("Add schedule").click();
        harness.run_steps(3);
        save(
            &mut harness,
            &out.join(format!("schedule-editor-{theme_name}.png")),
        );
    }
}

fn save(harness: &mut Harness<'static, GravekeeperApp<'static>>, path: &Path) {
    let image = harness.render().expect("render");
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(file, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(image.as_raw())
        .unwrap();
}
