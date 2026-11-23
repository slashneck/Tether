//! The window. Black and white, minimal, everything one click away. It only reads the engine's shared state and
//! sends commands, so a busy UI can never stall the video.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Align, Color32, FontFamily, FontId, Layout, RichText, Sense, Stroke, TextureHandle, TextureOptions,
    Ui,
};

use crate::engine::{Cmd, Phase, Shared};
use crate::look::{Filter, Fit, Look};
use crate::settings::{Hotkey, Quality, Settings, Source};

const BG: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x0A);
const SURFACE: Color32 = Color32::from_rgb(0x14, 0x14, 0x14);
const LINE: Color32 = Color32::from_rgb(0x1F, 0x1F, 0x1F);
const BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
const TEXT: Color32 = Color32::from_rgb(0xFA, 0xFA, 0xFA);
const SOFT: Color32 = Color32::from_rgb(0xBD, 0xBD, 0xBD);
const MUTED: Color32 = Color32::from_rgb(0x8F, 0x8F, 0x8F);
const BLACK: Color32 = Color32::from_rgb(0x0A, 0x0A, 0x0A);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Camera,
    Setup,
}

pub struct App {
    shared: Arc<Shared>,
    tx: Sender<Cmd>,
    /// Whether a tray icon exists (it lives in `TRAY`, so quitting can remove it from the menu handler).
    has_tray: bool,
    tab: Tab,
    texture: Option<TextureHandle>,
    texture_serial: u64,
    recording_hotkey: bool,
    /// "Custom" picked for resolution / frame rate (kept even when the chosen value is a standard one).
    custom_res: bool,
    custom_fps: bool,
}

pub fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    for (name, file, family) in [
        ("segoe", "segoeui.ttf", FontFamily::Proportional),
        ("segoe-semibold", "seguisb.ttf", FontFamily::Name("semibold".into())),
    ] {
        if let Ok(bytes) = std::fs::read(format!("{windir}\\Fonts\\{file}")) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    // Fall back to the regular face if the semibold one isn't there.
    let prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Name("semibold".into())).or_default().extend(prop);
    ctx.set_fonts(fonts);

    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE;
    v.faint_bg_color = SURFACE;
    v.override_text_color = Some(SOFT);
    v.hyperlink_color = TEXT;
    v.selection.bg_fill = TEXT;
    v.selection.stroke = Stroke::new(1.0, BLACK);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.weak_bg_fill = SURFACE;
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x2A, 0x2A, 0x2A);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x1C, 0x1C, 0x1C);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x55, 0x55, 0x55);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x55, 0x55, 0x55));
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x22, 0x22, 0x22);
    v.widgets.active.bg_fill = TEXT;
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.slider_width = 180.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.5));
    });
}

impl App {
    pub fn new(shared: Arc<Shared>, tx: Sender<Cmd>, ctx: &egui::Context) -> App {
        let tray = make_tray(ctx, shared.clone(), tx.clone());
        let has_tray = tray.is_some();
        TRAY.with(|t| *t.borrow_mut() = tray);
        App {
            shared,
            tx,
            has_tray,
            tab: Tab::Camera,
            texture: None,
            texture_serial: 0,
            recording_hotkey: false,
            custom_res: false,
            custom_fps: false,
        }
    }
}

/// Quits for real: the engine tells the phone to stop and removes Tether Camera. If anything hangs, the process
/// ends after a few seconds anyway (and Windows removes the camera with it).
pub fn quit_everything(tx: &Sender<Cmd>) {
    // The tray icon goes first, so it never lingers. (Called on the UI thread, which owns it.)
    TRAY.with(|t| t.borrow_mut().take());
    let _ = tx.send(Cmd::Quit);
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(4));
        crate::hard_exit();
    });
}

thread_local! {
    static TRAY: std::cell::RefCell<Option<tray_icon::TrayIcon>> = const { std::cell::RefCell::new(None) };
}

fn show_window(ctx: &egui::Context, shared: &Shared) {
    shared.minimized.store(false, std::sync::atomic::Ordering::Relaxed);
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

fn make_tray(ctx: &egui::Context, shared: Arc<Shared>, tx: Sender<Cmd>) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let img = image::load_from_memory(include_bytes!("../../../assets/icon-256.png")).ok()?;
    let img = img.resize(32, 32, image::imageops::FilterType::Triangle).to_rgba8();
    let icon = tray_icon::Icon::from_rgba(img.as_raw().clone(), img.width(), img.height()).ok()?;

    let menu = Menu::new();
    let open = MenuItem::new("Open Tether", true, None);
    let cover = MenuItem::new("Cover / uncover camera", true, None);
    let quit = MenuItem::new("Quit Tether (turns everything off)", true, None);
    menu.append_items(&[&open, &cover, &PredefinedMenuItem::separator(), &quit]).ok()?;
    let (open_id, cover_id, quit_id) = (open.id().clone(), cover.id().clone(), quit.id().clone());

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Tether")
        .with_icon(icon)
        .build()
        .ok()?;

    {
        let (ctx, shared, tx) = (ctx.clone(), shared.clone(), tx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == open_id {
                show_window(&ctx, &shared);
            } else if e.id == cover_id {
                let _ = tx.send(Cmd::ToggleCover);
            } else if e.id == quit_id {
                quit_everything(&tx);
            }
        }));
    }
    {
        let ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(&ctx, &shared);
            }
        }));
    }
    Some(tray)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // X hides to the tray; "Quit" in the tray menu really quits.
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.has_tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.shared.minimized.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            quit_everything(&self.tx);
        }
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        self.shared.minimized.store(minimized, std::sync::atomic::Ordering::Relaxed);

        // Newest preview frame → texture.
        {
            let p = self.shared.preview.lock().unwrap();
            if p.serial != self.texture_serial && p.width > 0 {
                self.texture_serial = p.serial;
                let img = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                match &mut self.texture {
                    Some(t) => t.set(img, TextureOptions::LINEAR),
                    None => self.texture = Some(ctx.load_texture("preview", img, TextureOptions::LINEAR)),
                }
            }
        }

        let mut settings = self.shared.settings.lock().unwrap().clone();
        let before = settings.clone();

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(16, 10)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| self.top_bar(ui));
        self.banner(ui);

        match self.tab {
            Tab::Camera => {
                egui::Panel::right("settings")
                    .resizable(false)
                    .exact_size(330.0)
                    .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(20, 16)).stroke(Stroke::new(1.0, LINE)))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_width(ui.available_width() - 8.0);
                            self.settings_panel(ui, &mut settings);
                        });
                    });
                egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(24)).show(ui, |ui| {
                    // The cards take the height they need at the bottom; the preview gets exactly what's left,
                    // so nothing ever scrolls.
                    let status = self.shared.status.lock().unwrap().clone();
                    egui::Panel::bottom("tools")
                        .resizable(false)
                        .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin { top: 20, ..Default::default() }))
                        .show(ui, |ui| {
                            ui.columns(3, |cols| {
                                self.framing_card(&mut cols[0], &mut settings);
                                self.filters_card(&mut cols[1], &mut settings);
                                self.cover_card(&mut cols[2], &mut settings, &status);
                            });
                        });
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(BG))
                        .show(ui, |ui| self.camera_view(ui, &mut settings));
                });
            }
            Tab::Setup => {
                egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(24)).show(ui, |ui| {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.setup_view(ui, &mut settings));
                });
            }
        }

        if settings != before {
            *self.shared.settings.lock().unwrap() = settings.clone();
            let _ = self.tx.send(Cmd::Settings(settings));
        }
    }
}

// ---------- small widgets ----------

fn heading(text: &str) -> RichText {
    RichText::new(text).family(FontFamily::Name("semibold".into())).color(TEXT)
}

/// A settings group: divider line, then a small caps title. The first group of a panel skips the line.
fn section(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, LINE);
    ui.add_space(14.0);
    section_title(ui, text);
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(MUTED).extra_letter_spacing(0.8));
    ui.add_space(2.0);
}

fn sub_label(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(12.5).color(SOFT));
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED));
}

/// The cards under the preview.
fn tool_card(ui: &mut Ui, title: &str, subtitle: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x10, 0x10, 0x10))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(230.0);
            ui.label(heading(title).size(15.0));
            ui.label(RichText::new(subtitle).size(12.0).color(MUTED));
            ui.add_space(6.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(r, 0.0, LINE);
            ui.add_space(8.0);
            add(ui);
        });
}

fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    let width = ui.available_width();
    egui::Frame::new().stroke(Stroke::new(1.0, BORDER)).corner_radius(6).inner_margin(3).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let n = options.len() as f32;
        let w = ((width - 8.0 - 3.0 * (n - 1.0)) / n).max(24.0);
        ui.horizontal(|ui| {
            for (v, label) in options {
                let sel = *value == *v;
                let text = RichText::new(*label).size(13.0).color(if sel { BLACK } else { MUTED });
                let b = egui::Button::new(text)
                    .fill(if sel { TEXT } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(4);
                if ui.add_sized([w, 28.0], b).clicked() && !sel {
                    *value = *v;
                    changed = true;
                }
            }
        });
    });
    changed
}

fn switch(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let r = rect.height() / 2.0;
    let painter = ui.painter();
    painter.rect_filled(rect, r, if *on { TEXT } else { Color32::from_rgb(0x2A, 0x2A, 0x2A) });
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    painter.circle_filled(pos2(x, rect.center().y), r - 3.5, if *on { BLACK } else { MUTED });
    resp
}

/// Draws a small line at the middle of a slider (where its default is). Returns true on double-click: reset it.
fn center_mark(ui: &Ui, slider: &egui::Response) -> bool {
    let r = slider.rect;
    let x = r.center().x;
    ui.painter().line_segment(
        [pos2(x, r.center().y - 7.0), pos2(x, r.center().y + 7.0)],
        Stroke::new(1.5, Color32::from_rgb(0x6A, 0x6A, 0x6A)),
    );
    slider.double_clicked()
}

fn toggle_row(ui: &mut Ui, label: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(SOFT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            switch(ui, on);
        });
    });
}

fn slider_row(ui: &mut Ui, label: &str, value: &mut i32) {
    ui.horizontal(|ui| {
        ui.add_sized([86.0, 20.0], egui::Label::new(RichText::new(label).color(SOFT)));
        ui.spacing_mut().slider_width = ui.available_width() - 44.0;
        let r = ui.add(egui::Slider::new(value, -100..=100).show_value(false));
        if center_mark(ui, &r) {
            *value = 0;
        }
        ui.label(RichText::new(format!("{value}")).size(12.0).color(MUTED));
    });
}

fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(BLACK).family(FontFamily::Name("semibold".into())))
        .fill(TEXT)
        .stroke(Stroke::NONE)
        .corner_radius(6)
        .min_size(vec2(0.0, 36.0))
}

fn outline(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(TEXT))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(6)
        .min_size(vec2(0.0, 36.0))
}

fn paint_mark(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter();
    let u = size / 24.0;
    let c = pos2(rect.left() + 12.0 * u, rect.top() + 10.0 * u);
    p.circle_stroke(c, 6.0 * u, Stroke::new(2.0 * u, TEXT));
    p.circle_filled(c, 1.8 * u, TEXT);
    p.line_segment([pos2(c.x, rect.top() + 16.0 * u), pos2(c.x, rect.top() + 22.0 * u)], Stroke::new(2.0 * u, TEXT));
}

impl App {
    fn top_bar(&mut self, ui: &mut Ui) {
        let status = self.shared.status.lock().unwrap().clone();
        ui.horizontal(|ui| {
            paint_mark(ui, 20.0);
            ui.label(heading("Tether").size(15.0));
            ui.add_space(18.0);
            for (tab, label) in [(Tab::Camera, "Camera"), (Tab::Setup, "Setup")] {
                let sel = self.tab == tab;
                let text = RichText::new(label).size(13.5).color(if sel { TEXT } else { MUTED });
                let b = egui::Button::new(text)
                    .fill(if sel { Color32::from_rgb(0x1C, 0x1C, 0x1C) } else { Color32::TRANSPARENT })
                    .stroke(Stroke::NONE)
                    .corner_radius(6);
                if ui.add_sized([76.0, 28.0], b).clicked() {
                    self.tab = tab;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (dot, text) = if status.in_use {
                    (TEXT, "Tether Camera is in use")
                } else {
                    (MUTED, "Pick \"Tether Camera\" in Discord, OBS, Zoom…")
                };
                ui.label(RichText::new(text).size(12.5).color(if status.in_use { SOFT } else { MUTED }));
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, dot);
            });
        });
    }

    // ---------- camera tab ----------

    fn camera_view(&mut self, ui: &mut Ui, settings: &mut Settings) {
        let status = self.shared.status.lock().unwrap().clone();
        // 16:9, as large as fits, centred. The buttons float on its bottom edge.
        let avail = ui.available_size();
        let h = (avail.x * 9.0 / 16.0).min(avail.y).max(120.0);
        let w = h * 16.0 / 9.0;
        let (row, _) = ui.allocate_exact_size(vec2(avail.x, h), Sense::hover());
        let rect = egui::Rect::from_center_size(row.center(), vec2(w, h));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 8.0, SURFACE);

        let live = status.phase == Phase::Live;
        let showing_cover = status.covered || (!live && settings.cover_on_disconnect);
        if let Some(tex) = &self.texture {
            if live || showing_cover {
                let ts = tex.size_vec2();
                let scale = (w / ts.x).min(h / ts.y);
                let size = ts * scale;
                let img_rect = egui::Rect::from_center_size(rect.center(), size);
                painter.image(tex.id(), img_rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        }
        if !live {
            let msg = match &status.phase {
                Phase::Searching(m) => m.clone(),
                Phase::Live => String::new(),
            };
            // Waiting for a phone over Wi‑Fi: the pairing QR code, big and crisp, on its own white card.
            if let Some(url) = status.pair_url.as_ref().filter(|_| settings.source == Source::Browser) {
                if let Ok(qr) = qrcode::QrCode::new(url.as_bytes()) {
                    let n = qr.width();
                    let card = (h * 0.56).min(w * 0.4);
                    let card_rect = egui::Rect::from_center_size(rect.center() - vec2(0.0, 26.0), vec2(card, card));
                    painter.rect_filled(card_rect.expand(6.0), 14.0, BG);
                    painter.rect_filled(card_rect, 10.0, Color32::WHITE);
                    let quiet = 3.0;
                    let cell = card / (n as f32 + quiet * 2.0);
                    let origin = card_rect.min + vec2(cell * quiet, cell * quiet);
                    let colors = qr.to_colors();
                    for y in 0..n {
                        for x in 0..n {
                            if colors[y * n + x] == qrcode::Color::Dark {
                                let r = egui::Rect::from_min_size(origin + vec2(x as f32 * cell, y as f32 * cell), vec2(cell + 0.5, cell + 0.5));
                                painter.rect_filled(r, 0.0, BLACK);
                            }
                        }
                    }
                }
            }
            let galley_pos = pos2(rect.center().x, rect.bottom() - 84.0);
            painter.text(galley_pos, egui::Align2::CENTER_CENTER, msg, FontId::proportional(15.0), TEXT);
        }

        // Status pill.
        let pill = if status.covered {
            "COVERED".to_string()
        } else if status.paused {
            "PAUSED".to_string()
        } else if live {
            match status.actual {
                Some((_, ph, f)) => format!("LIVE · {} · {}p{}", status.link, ph, f),
                None => format!("LIVE · {}", status.link),
            }
        } else {
            "NOT CONNECTED".to_string()
        };
        let font = FontId::proportional(12.0);
        let galley = ui.painter().layout_no_wrap(pill, font, TEXT);
        let pill_rect = egui::Rect::from_min_size(rect.min + vec2(14.0, 14.0), galley.size() + vec2(30.0, 12.0));
        painter.rect(pill_rect, 99.0, BG, Stroke::new(1.0, Color32::from_rgb(0x26, 0x26, 0x26)), egui::StrokeKind::Inside);
        painter.circle_filled(pill_rect.left_center() + vec2(12.0, 0.0), 3.5, if live && !status.covered { TEXT } else { MUTED });
        painter.galley(pill_rect.min + vec2(22.0, 6.0), galley, TEXT);

        // Toolbar floating on the preview's bottom edge, on dark glass so it reads over any picture.
        let bar = egui::Rect::from_min_max(pos2(rect.left() + 14.0, rect.bottom() - 52.0), pos2(rect.right() - 14.0, rect.bottom() - 14.0));
        let glass = |text: &str, primary_look: bool| {
            let (fill, color) = if primary_look { (TEXT, BLACK) } else { (Color32::from_rgba_unmultiplied(10, 10, 10, 215), TEXT) };
            egui::Button::new(RichText::new(text.to_string()).color(color).family(FontFamily::Name("semibold".into())))
                .fill(fill)
                .stroke(Stroke::new(1.0, if primary_look { TEXT } else { Color32::from_rgb(0x3A, 0x3A, 0x3A) }))
                .corner_radius(8)
                .min_size(vec2(0.0, 36.0))
        };
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar).layout(Layout::left_to_right(Align::Center)), |ui| {
            if ui.add(glass(if status.paused { "  Resume  " } else { "  Pause  " }, true)).clicked() {
                let _ = self.tx.send(Cmd::TogglePause);
            }
            let cover_label = match &settings.hotkey {
                Some(hk) if !status.covered => format!("  Cover  ·  {}  ", hk.label()),
                _ if status.covered => "  Uncover  ".to_string(),
                _ => "  Cover  ".to_string(),
            };
            if ui.add(glass(&cover_label, false)).clicked() {
                let _ = self.tx.send(Cmd::ToggleCover);
            }
            if ui.add(glass(if settings.mirror { "  Mirrored  " } else { "  Mirror  " }, false)).clicked() {
                settings.mirror = !settings.mirror;
            }
            if ui.add(glass("  Rotate  ", false)).clicked() {
                settings.rotation = (settings.rotation + 90) % 360;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if live {
                    let mut stats = format!("{:.0} fps", status.fps);
                    if status.mbps > 0.0 {
                        stats += &format!(" · {:.1} Mbit/s · decode {:.1} ms", status.mbps, status.decode_ms);
                    }
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(10, 10, 10, 215))
                        .corner_radius(99)
                        .inner_margin(egui::Margin::symmetric(12, 6))
                        .show(ui, |ui| ui.label(RichText::new(stats).size(12.0).color(SOFT)));
                }
            });
        });
    }

    fn settings_panel(&mut self, ui: &mut Ui, s: &mut Settings) {
        let status = self.shared.status.lock().unwrap().clone();

        section_title(ui, "SOURCE");
        let current = match &s.source {
            Source::Phone => "Phone (USB)".to_string(),
            Source::Browser => "Phone over Wi‑Fi (any phone)".to_string(),
            Source::Webcam { name, .. } => name.clone(),
        };
        let combo = egui::ComboBox::from_id_salt("source").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
            ui.selectable_value(&mut s.source, Source::Phone, "Phone (USB)");
            ui.selectable_value(&mut s.source, Source::Browser, "Phone over Wi‑Fi (any phone)");
            for cam in &status.webcams {
                ui.selectable_value(
                    &mut s.source,
                    Source::Webcam { link: cam.link.clone(), name: cam.name.clone() },
                    &cam.name,
                );
            }
        });
        if combo.response.clicked() {
            let _ = self.tx.send(Cmd::RefreshWebcams);
        }

        if s.source == Source::Browser {
            ui.add_space(6.0);
            hint(ui, "Works with any phone, iPhone included: scan the code in the preview with the phone's camera. Phone and PC must be on the same Wi‑Fi.");
            hint(ui, "Camera, flip and light are on the phone's screen; it picks the best quality it can send.");
            section(ui, "ORIENTATION");
            segmented(ui, &mut s.rotation, &[(0, "0°"), (90, "90°"), (180, "180°"), (270, "270°")]);
            toggle_row(ui, "Mirror", &mut s.mirror);
            return;
        }

        let phone = s.source == Source::Phone;
        if phone {
            if !status.device.is_empty() && status.phase == Phase::Live {
                ui.horizontal(|ui| {
                    ui.label(heading(&status.device).size(14.0));
                    if let Some(b) = status.battery {
                        ui.label(RichText::new(format!("· {b}% battery")).size(12.5).color(MUTED));
                    }
                });
            }

            section(ui, "CAMERA");
            let back = status.cameras.iter().find(|c| c.facing == "back").map_or("0".to_string(), |c| c.id.clone());
            let front = status.cameras.iter().find(|c| c.facing == "front").map_or("1".to_string(), |c| c.id.clone());
            let mut which = if s.camera == front { 1 } else { 0 };
            if segmented(ui, &mut which, &[(0, "Back"), (1, "Front")]) {
                s.camera = if which == 1 { front } else { back };
            }
        }

        section(ui, "QUALITY");
        #[derive(PartialEq, Clone, Copy)]
        enum Preset {
            Saver,
            Balanced,
            Best,
            Custom,
        }
        let preset = match (s.height, s.fps, s.quality) {
            (720, 30, Quality::Low) => Preset::Saver,
            (1080, 30, Quality::Normal) => Preset::Balanced,
            (1080, 60, Quality::High) => Preset::Best,
            _ => Preset::Custom,
        };
        let mut p = preset;
        if segmented(ui, &mut p, &[(Preset::Saver, "Battery saver"), (Preset::Balanced, "Balanced"), (Preset::Best, "Best")]) {
            self.custom_res = false;
            self.custom_fps = false;
            match p {
                Preset::Saver => {
                    (s.width, s.height, s.fps, s.quality) = (1280, 720, 30, Quality::Low);
                    s.phone_preview = false;
                    s.phone_dim = true;
                }
                Preset::Balanced => (s.width, s.height, s.fps, s.quality) = (1920, 1080, 30, Quality::Normal),
                Preset::Best => (s.width, s.height, s.fps, s.quality) = (1920, 1080, 60, Quality::High),
                Preset::Custom => {}
            }
        }

        // Resolution: Custom | 720p | 1080p. Custom opens the full list.
        sub_label(ui, "Resolution");
        let standard_res = (s.width, s.height) == (1280, 720) || (s.width, s.height) == (1920, 1080);
        let mut res = if self.custom_res || !standard_res { 0 } else { s.height };
        if segmented(ui, &mut res, &[(0, "Custom"), (720, "720p"), (1080, "1080p")]) {
            self.custom_res = res == 0;
            match res {
                720 => (s.width, s.height) = (1280, 720),
                1080 => (s.width, s.height) = (1920, 1080),
                _ => {}
            }
        }
        if res == 0 {
            const SIZES: [(u32, u32, &str); 8] = [
                (854, 480, "480p · 854 × 480"),
                (960, 540, "540p · 960 × 540"),
                (1280, 720, "720p · 1280 × 720"),
                (1600, 900, "900p · 1600 × 900"),
                (1920, 1080, "1080p · 1920 × 1080"),
                (2560, 1440, "1440p · 2560 × 1440"),
                (3840, 2160, "4K · 3840 × 2160"),
                (1080, 1920, "Portrait · 1080 × 1920"),
            ];
            let current = SIZES
                .iter()
                .find(|x| (x.0, x.1) == (s.width, s.height))
                .map_or(format!("{} × {}", s.width, s.height), |x| x.2.to_string());
            egui::ComboBox::from_id_salt("custom-res").selected_text(current).width(ui.available_width()).show_ui(ui, |ui| {
                for (w, h, label) in SIZES {
                    if ui.selectable_label((s.width, s.height) == (w, h), label).clicked() {
                        (s.width, s.height) = (w, h);
                    }
                }
            });
            if s.height > 1080 || s.width > 1920 {
                hint(ui, "Apps get up to 1080p. A bigger source gives sharper zoom.");
            }
        }

        // Frame rate: Custom | 30 | 60.
        sub_label(ui, "Frame rate");
        let mut fps = if self.custom_fps || !(s.fps == 30 || s.fps == 60) { 0 } else { s.fps };
        if segmented(ui, &mut fps, &[(0, "Custom"), (30, "30 fps"), (60, "60 fps")]) {
            self.custom_fps = fps == 0;
            if fps != 0 {
                s.fps = fps;
            }
        }
        if fps == 0 {
            ui.horizontal(|ui| {
                ui.spacing_mut().slider_width = ui.available_width() - 70.0;
                ui.add(egui::Slider::new(&mut s.fps, 5..=120).show_value(false));
                ui.label(RichText::new(format!("{} fps", s.fps)).size(12.5).color(SOFT));
            });
        }
        let max = status.cameras.iter().find(|c| c.id == s.camera).map(|c| c.max_fps);
        if let Some(max) = max.filter(|m| phone && *m < s.fps) {
            hint(ui, &format!("This phone's camera tops out at {max} fps."));
        }

        sub_label(ui, "Bitrate");
        segmented(ui, &mut s.quality, &[(Quality::Low, "Light"), (Quality::Normal, "Normal"), (Quality::High, "Sharp")]);
        hint(ui, &format!("≈ {:.1} Mbit/s", s.bitrate() as f32 / 1e6));

        if phone {
            section(ui, "PHONE");
            let torch_ok = status.cameras.iter().find(|c| c.id == s.camera).map_or(true, |c| c.torch);
            if torch_ok {
                toggle_row(ui, "Flashlight", &mut s.torch);
            }
            toggle_row(ui, "Preview on phone screen", &mut s.phone_preview);
            toggle_row(ui, "Dim phone screen when idle", &mut s.phone_dim);
        }

        section(ui, "ORIENTATION");
        segmented(ui, &mut s.rotation, &[(0, "0°"), (90, "90°"), (180, "180°"), (270, "270°")]);
        toggle_row(ui, "Mirror", &mut s.mirror);
        ui.add_space(20.0);
    }

    // ---------- cards under the preview ----------

    fn framing_card(&mut self, ui: &mut Ui, s: &mut Settings) {
        tool_card(ui, "Zoom & framing", "Crop in and choose how it fills the frame", |ui| {
            ui.horizontal(|ui| {
                ui.add_sized([70.0, 20.0], egui::Label::new(RichText::new("Zoom").color(SOFT)));
                ui.spacing_mut().slider_width = ui.available_width() - 40.0;
                ui.add(egui::Slider::new(&mut s.look.zoom, 1.0..=4.0).show_value(false));
                ui.label(RichText::new(format!("{:.1}x", s.look.zoom)).size(12.0).color(MUTED));
            });
            ui.add_enabled_ui(s.look.zoom > 1.01, |ui| {
                for (label, v) in [("Left / right", &mut s.look.pan_x), ("Up / down", &mut s.look.pan_y)] {
                    ui.horizontal(|ui| {
                        ui.add_sized([70.0, 20.0], egui::Label::new(RichText::new(label).color(SOFT)));
                        ui.spacing_mut().slider_width = ui.available_width() - 4.0;
                        let r = ui.add(egui::Slider::new(v, -1.0..=1.0).show_value(false));
                        if center_mark(ui, &r) {
                            *v = 0.0;
                        }
                    });
                }
            });
            ui.add_space(4.0);
            segmented(ui, &mut s.look.fit, &[(Fit::Fill, "Fill"), (Fit::Fit, "Fit"), (Fit::Stretch, "Stretch")]);
            hint(
                ui,
                match s.look.fit {
                    Fit::Fill => "Fills the frame, trims the edges.",
                    Fit::Fit => "Shows everything, black bars if needed.",
                    Fit::Stretch => "Fills the frame by stretching.",
                },
            );
        });
    }

    fn filters_card(&mut self, ui: &mut Ui, s: &mut Settings) {
        tool_card(ui, "Filters", "Colour and light", |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for f in Filter::ALL {
                    let sel = s.look.filter == f;
                    let b = egui::Button::new(RichText::new(f.label()).size(12.5).color(if sel { BLACK } else { SOFT }))
                        .fill(if sel { TEXT } else { Color32::TRANSPARENT })
                        .stroke(Stroke::new(1.0, if sel { TEXT } else { BORDER }))
                        .corner_radius(99);
                    if ui.add(b).clicked() {
                        s.look.filter = f;
                    }
                }
            });
            ui.add_space(6.0);
            slider_row(ui, "Brightness", &mut s.look.brightness);
            slider_row(ui, "Contrast", &mut s.look.contrast);
            slider_row(ui, "Saturation", &mut s.look.saturation);
            if s.look != Look::default() {
                ui.add_space(4.0);
                if ui.add(outline("  Reset zoom & filters  ")).clicked() {
                    s.look = Look::default();
                }
            }
        });
    }

    fn cover_card(&mut self, ui: &mut Ui, s: &mut Settings, status: &crate::engine::Status) {
        tool_card(ui, "Cover", "Hide yourself instantly", |ui| {
            sub_label(ui, "Hotkey");
            ui.horizontal(|ui| {
                let label = if self.recording_hotkey {
                    "Press keys…  (Esc cancels)".to_string()
                } else {
                    s.hotkey.as_ref().map_or("Set hotkey".to_string(), |h| h.label())
                };
                if ui.add(outline(&format!("  {label}  "))).clicked() {
                    self.recording_hotkey = !self.recording_hotkey;
                }
                if s.hotkey.is_some() && !self.recording_hotkey && ui.add(outline("  Clear  ")).clicked() {
                    s.hotkey = None;
                }
            });
            if self.recording_hotkey {
                if let Some(hk) = capture_hotkey(ui) {
                    self.recording_hotkey = false;
                    if hk.vk != 0 {
                        s.hotkey = Some(hk);
                    }
                }
            }
            if let Some(err) = &status.hotkey_error {
                ui.label(RichText::new(err).size(12.0).color(TEXT));
            }
            sub_label(ui, "Picture");
            let name = s
                .cover_file
                .as_ref()
                .and_then(|p| p.file_name())
                .map_or("Built-in card".to_string(), |n| n.to_string_lossy().to_string());
            ui.label(RichText::new(name).color(TEXT));
            ui.horizontal(|ui| {
                if ui.add(outline("  Choose GIF / image…  ")).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("GIF or image", &["gif", "png", "jpg", "jpeg", "webp"])
                        .pick_file()
                    {
                        // Keep a copy, so the cover still works if the original gets moved or deleted.
                        let stored = crate::engine::cover_store_path(&path)
                            .filter(|dst| std::fs::create_dir_all(dst.parent().unwrap()).is_ok() && std::fs::copy(&path, dst).is_ok());
                        s.cover_file = Some(stored.unwrap_or(path));
                    }
                }
                if s.cover_file.is_some() && ui.add(outline("  Built-in  ")).clicked() {
                    s.cover_file = None;
                }
            });
            if let Some(err) = &status.cover_error {
                ui.label(RichText::new(err).size(12.0).color(TEXT));
            }
            ui.add_space(4.0);
            toggle_row(ui, "Show when connection drops", &mut s.cover_on_disconnect);
        });
    }

    // ---------- setup tab ----------

    /// One line under the top bar for things that need a click: a new Tether, or an outdated phone app.
    fn banner(&mut self, ui: &mut Ui) {
        use crate::updater::{is_newer, UpdateState, VERSION};
        let status = self.shared.status.lock().unwrap().clone();
        let phone_old = status.phase == Phase::Live
            && status.link == "USB"
            && status.phone_app.as_deref().is_some_and(|v| is_newer(VERSION, v));
        let camera_busy = status.camera_repair.as_ref().is_some_and(|c| c.0);
        let (text, button, cmd): (String, Option<&str>, Option<Cmd>) = match &status.update {
            // Without Tether Camera nothing else matters, so this comes first.
            _ if status.camera_repair.is_some() => (
                status.camera_repair.clone().unwrap_or_default().1,
                Some(if camera_busy { "Repairing…" } else { "Repair" }),
                Some(Cmd::RepairCamera),
            ),
            UpdateState::Available(rel) => (
                format!("Tether {} is out. You have {VERSION}.", rel.version),
                Some("Update now"),
                Some(Cmd::InstallUpdate),
            ),
            UpdateState::Downloading(p) => (format!("Downloading the update… {:.0}%", p * 100.0), None, None),
            _ if phone_old => (
                format!(
                    "The Tether app on your phone is {} (this PC has {VERSION}).",
                    status.phone_app.clone().unwrap_or_default()
                ),
                Some("Update phone app"),
                Some(Cmd::InstallOnPhone),
            ),
            _ => return,
        };
        egui::Panel::top("banner")
            .frame(egui::Frame::new().fill(Color32::from_rgb(0x16, 0x16, 0x16)).inner_margin(egui::Margin::symmetric(20, 8)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, TEXT);
                    ui.label(RichText::new(text).color(TEXT));
                    if let Some(msg) = status.install.as_ref().filter(|_| phone_old && status.camera_repair.is_none()).map(|i| i.1.clone()) {
                        ui.label(RichText::new(msg).size(12.5).color(MUTED));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let (Some(label), Some(cmd)) = (button, cmd) {
                            let busy = camera_busy || status.install.as_ref().is_some_and(|i| i.0);
                            if ui.add_enabled(!busy, primary(&format!("  {label}  "))).clicked() {
                                let _ = self.tx.send(cmd);
                            }
                        }
                    });
                });
            });
    }

    fn setup_view(&mut self, ui: &mut Ui, settings: &mut Settings) {
        let status = self.shared.status.lock().unwrap().clone();
        // A centred reading column, wide enough for two cards side by side.
        let width = ui.available_width().min(820.0);
        let pad = ((ui.available_width() - width) / 2.0).max(0.0);
        ui.horizontal(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.add_space(8.0);
                ui.label(heading("Set up Tether").size(28.0));
                ui.add_space(2.0);
                ui.label(RichText::new("Three steps, once per phone. After that: plug in and you're live.").size(14.5).color(MUTED));
                ui.add_space(28.0);

                // ---- 1 ----
                card(ui, |ui| {
                    card_header(ui, "1", "Turn on USB debugging", "Lets this PC talk to your phone over the cable.");
                    numbered(ui, 1, "Open Settings → About phone.");
                    numbered(ui, 2, "Tap \"Build number\" 7 times. (On some phones it's \"OS version\" or \"Software version\".)");
                    numbered(ui, 3, "Go to Settings → Developer options and turn on \"USB debugging\".");
                    numbered(ui, 4, "Plug the phone in. Tap \"Allow\" on the phone and tick \"Always allow\".");
                });
                ui.add_space(16.0);

                // ---- 2 ----
                card(ui, |ui| {
                    card_header(ui, "2", "Install Tether on your phone", "Pick one of the two ways.");
                    ui.columns(2, |cols| {
                        option_box(&mut cols[0], |ui| {
                            ui.label(RichText::new("EASIEST").size(11.0).color(MUTED).extra_letter_spacing(0.8));
                            ui.label(heading("From this PC").size(16.0));
                            ui.add_space(4.0);
                            ui.label(RichText::new("One click. Also updates the phone app later.").color(SOFT));
                            ui.add_space(10.0);
                            let busy = status.install.as_ref().is_some_and(|i| i.0);
                            if ui.add_enabled(!busy, primary("   Install on phone   ")).clicked() {
                                let _ = self.tx.send(Cmd::InstallOnPhone);
                            }
                            if let Some((_, msg)) = &status.install {
                                ui.add_space(6.0);
                                ui.label(RichText::new(msg).color(TEXT));
                            }
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(
                                    "If your phone blocks this, Tether puts the app in the phone's Download folder \
                                     and opens it. Tap Tether.apk → Install.",
                                )
                                .size(12.5)
                                .color(MUTED),
                            );
                        });
                        option_box(&mut cols[1], |ui| {
                            ui.label(RichText::new("MANUAL").size(11.0).color(MUTED).extra_letter_spacing(0.8));
                            ui.label(heading("Download the APK").size(16.0));
                            ui.add_space(6.0);
                            numbered(ui, 1, "On the phone, open:");
                            ui.horizontal(|ui| {
                                ui.add_space(32.0);
                                ui.hyperlink_to(
                                    RichText::new("github.com/slashneck/Tether/releases").underline(),
                                    "https://github.com/slashneck/Tether/releases",
                                );
                            });
                            numbered(ui, 2, "Download the newest Tether-x.y.z.apk.");
                            numbered(ui, 3, "Open it. Allow \"Install unknown apps\" if asked.");
                            numbered(ui, 4, "Tap Install, then open Tether.");
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("No internet on the phone? Copy tether.apk from the Tether folder over USB.")
                                    .size(12.5)
                                    .color(MUTED),
                            );
                            ui.add_space(4.0);
                            if ui.add(outline("  Open the Tether folder  ")).clicked() {
                                if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
                                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                                }
                            }
                        });
                    });
                });
                ui.add_space(16.0);

                // ---- any phone over Wi‑Fi ----
                card(ui, |ui| {
                    card_header(ui, "+", "No cable? Any phone over Wi‑Fi", "iPhone too. Nothing to install, it runs in the phone's browser.");
                    numbered(ui, 1, "Under Source (Camera tab), pick “Phone over Wi‑Fi (any phone)”. A QR code appears in the preview.");
                    numbered(ui, 2, "Scan it with the phone's camera (or Google Lens) and open the link in Chrome or Safari.");
                    numbered(ui, 3, "Tap Start camera and allow the camera. Keep the page open; the screen can be dimmed.");
                    ui.add_space(4.0);
                    hint(ui, "Phone and PC must be on the same network. A guest Wi‑Fi keeps them apart; Tether tells you when that's the case.");
                });
                ui.add_space(16.0);

                // ---- 3 ----
                card(ui, |ui| {
                    card_header(ui, "3", "Pick Tether Camera in your app", "Do it once; it stays picked.");
                    kv_row(ui, "Discord", "Settings → Voice & Video → Camera → Tether Camera");
                    divider(ui);
                    kv_row(ui, "OBS", "Sources → + → Video Capture Device → Tether Camera");
                    divider(ui);
                    kv_row(ui, "Zoom, Teams, browser", "Camera settings → Tether Camera");
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(
                            "Keep Tether running (the tray is fine). Switching phone ↔ webcam, covering or filters \
                             happen here; your apps never notice.",
                        )
                        .size(12.5)
                        .color(MUTED),
                    );
                });

                // ---- troubleshooting ----
                ui.add_space(36.0);
                ui.label(heading("If something's off").size(18.0));
                ui.add_space(10.0);
                card(ui, |ui| {
                    kv_row(ui, "\"Plug in your phone\", but it is", "Try another cable or port (some only charge). Set the phone's USB mode to File transfer.");
                    divider(ui);
                    kv_row(ui, "\"Tap Allow\"", "The USB debugging prompt is waiting on the phone's screen.");
                    divider(ui);
                    kv_row(ui, "Freezes when the screen is off", "Let Tether run without battery restrictions: long-press the app → App info → Battery.");
                    divider(ui);
                    kv_row(ui, "Hotkey does nothing", "Another app uses the same keys. Pick another one in the Camera tab.");
                });

                // ---- about / updates ----
                ui.add_space(36.0);
                ui.label(heading("About").size(18.0));
                ui.add_space(10.0);
                card(ui, |ui| {
                    use crate::updater::{UpdateState, VERSION};
                    ui.horizontal(|ui| {
                        paint_mark(ui, 28.0);
                        ui.vertical(|ui| {
                            ui.label(heading(&format!("Tether {VERSION}")).size(16.0));
                            ui.hyperlink_to(
                                RichText::new("github.com/slashneck/Tether").size(12.5).color(MUTED),
                                "https://github.com/slashneck/Tether",
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let busy = matches!(status.update, UpdateState::Checking | UpdateState::Downloading(_));
                            if let UpdateState::Available(rel) = &status.update {
                                if ui.add(primary(&format!("  Update to {}  ", rel.version))).clicked() {
                                    let _ = self.tx.send(Cmd::InstallUpdate);
                                }
                            } else if ui.add_enabled(!busy, outline("  Check for updates  ")).clicked() {
                                let _ = self.tx.send(Cmd::CheckUpdate { quiet: false });
                            }
                        });
                    });
                    let note = match &status.update {
                        UpdateState::Idle => None,
                        UpdateState::Checking => Some("Checking GitHub…".to_string()),
                        UpdateState::UpToDate => Some("You have the newest version.".to_string()),
                        UpdateState::Available(rel) => Some(format!("Tether {} is available.", rel.version)),
                        UpdateState::Downloading(p) => Some(format!("Downloading… {:.0}%", p * 100.0)),
                        UpdateState::Error(e) => Some(e.clone()),
                    };
                    if let Some(note) = note {
                        ui.add_space(6.0);
                        ui.label(RichText::new(note).color(SOFT));
                    }
                    divider(ui);
                    toggle_row(ui, "Check for updates automatically", &mut settings.auto_update);
                    hint(ui, "Asks GitHub for the newest release at start and every 6 hours. Nothing else is sent.");
                });
                ui.add_space(40.0);
            });
        });
    }
}

// ---------- setup page building blocks ----------

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x10, 0x10, 0x10))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(12)
        .inner_margin(egui::Margin::symmetric(26, 22))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn option_box(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(250.0);
            add(ui);
        });
}

fn card_header(ui: &mut Ui, n: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 15.0, TEXT);
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, n, FontId::new(14.0, FontFamily::Name("semibold".into())), BLACK);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(heading(title).size(18.0));
            ui.label(RichText::new(subtitle).size(13.0).color(MUTED));
        });
    });
    ui.add_space(14.0);
    divider(ui);
}

fn numbered(ui: &mut Ui, n: usize, text: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
        ui.painter().circle_stroke(r.center(), 10.0, Stroke::new(1.0, BORDER));
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, n.to_string(), FontId::proportional(11.5), SOFT);
        ui.add_space(4.0);
        ui.add(egui::Label::new(RichText::new(text).size(14.0).color(TEXT)).wrap());
    });
    ui.add_space(4.0);
}

fn kv_row(ui: &mut Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([210.0, 20.0], egui::Label::new(heading(key).size(14.0)).wrap());
        ui.add(egui::Label::new(RichText::new(value).size(13.5).color(SOFT)).wrap());
    });
}

fn divider(ui: &mut Ui) {
    ui.add_space(8.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, LINE);
    ui.add_space(8.0);
}

/// While recording a hotkey: the first non-modifier key pressed (with its modifiers). Esc cancels (vk 0).
fn capture_hotkey(ui: &mut Ui) -> Option<Hotkey> {
    ui.input(|i| {
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                if *key == egui::Key::Escape {
                    return Some(Hotkey { ctrl: false, shift: false, alt: false, win: false, vk: 0, key: String::new() });
                }
                let name = key.name();
                if let Some(vk) = crate::hotkey::vk_from_name(name) {
                    let is_f_key = (0x70..=0x87).contains(&vk);
                    if !is_f_key && !(modifiers.ctrl || modifiers.alt || modifiers.shift) {
                        continue; // a bare letter would fire while typing in Discord
                    }
                    return Some(Hotkey {
                        ctrl: modifiers.ctrl,
                        shift: modifiers.shift,
                        alt: modifiers.alt,
                        win: false,
                        vk,
                        key: name.to_uppercase(),
                    });
                }
            }
        }
        None
    })
}
