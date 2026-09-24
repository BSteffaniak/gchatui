//! bmux rich-content interaction and presentation adapter.
use super::{
    ACCENT, Action, App, AppMessage, Button, ButtonComponent, ButtonOutcome, ButtonState, Cell,
    Command, CommandKey, EmptyComponent, Event, FocusedPane, HitId, HitRegion, HitRole, Insets,
    LayoutCx, Line, LocalRect, MouseButton, MouseEventKind, PaintCx, Rect, SURFACE, Size, Style,
    TEXT, TranscriptItem, Update, VirtualListState, button_styles, conversation_content_area,
    paint_component, raster_area, transcript_list,
};
use bmux_tui::image::{ImageContribution, ImageKey, ImageLifecycle, ImagePayload, ImagePlacement};
use bmux_tui_components::modal_frame::{ModalFrame, ModalFrameComponent, ModalSizing, ModalTheme};

pub(super) struct Viewer {
    content: crate::model::RichContent,
    scroll: VirtualListState<String>,
    close: ButtonState,
    links: Vec<ButtonState>,
    selected: usize,
}

impl App {
    pub(super) fn reconcile_viewer(&mut self, items: &[(String, crate::model::RichContent)]) {
        if self.viewer.as_ref().is_some_and(|viewer| {
            !items.iter().any(|(_, content)| {
                content.image_url == viewer.content.image_url
                    && content.title == viewer.content.title
                    && content.text == viewer.content.text
            })
        }) {
            self.viewer = None;
        }
    }

    fn wanted_images(&self) -> Vec<String> {
        if let Some(viewer) = &self.viewer {
            return viewer.content.image_url.iter().cloned().collect();
        }
        let start = self.conversation_view.scroll.vertical_offset();
        let end = start.saturating_add(u64::from(
            conversation_content_area(self.conversation_pane.area).height,
        ));
        self.rich_items
            .iter()
            .filter(|(key, _)| {
                self.conversation_view
                    .item_offset(key)
                    .is_some_and(|offset| offset < end && offset.saturating_add(7) > start)
            })
            .filter_map(|(_, content)| content.image_url.clone())
            .collect()
    }

    pub(super) fn merge_images(&mut self, images: crate::rich_content::Images) {
        for url in images.keys() {
            if !self.image_recency.contains(url) {
                self.image_recency.insert(0, url.clone());
            }
        }
        self.images.extend(images);
        self.trim_images();
    }

    pub(super) fn trim_images(&mut self) {
        let wanted = self.wanted_images();
        while self.images.len() > 32 {
            let Some(index) = self
                .image_recency
                .iter()
                .position(|url| self.images.contains_key(url) && !wanted.contains(url))
            else {
                break;
            };
            let url = self.image_recency.remove(index);
            self.images.remove(&url);
            tracing::info!(target: "gchatui::diagnostics", cached_count = self.images.len(), reason = "capacity", "image_cache_evicted");
        }
    }

    pub(super) fn schedule_images(&mut self) -> Update<AppMessage> {
        let wanted = self.wanted_images();
        let ready = wanted
            .iter()
            .filter(|url| matches!(self.images.get(*url), Some(Ok(_))))
            .count();
        let missing = wanted
            .iter()
            .filter(|url| !self.images.contains_key(*url))
            .count();
        let state = (wanted.len(), ready, missing, self.image_loading.is_some());
        if self.image_cache_report != Some(state) {
            tracing::info!(target: "gchatui::diagnostics", visible = state.0, ready, missing, in_flight = state.3, "image_cache_state");
            self.image_cache_report = Some(state);
        }
        self.image_recency
            .retain(|url| self.images.contains_key(url) || wanted.contains(url));
        for url in &wanted {
            self.image_recency.retain(|entry| entry != url);
            self.image_recency.push(url.clone());
        }
        if self.image_loading.is_some() {
            return Update::none();
        }
        let urls = wanted
            .into_iter()
            .filter(|url| !self.images.contains_key(url))
            .take(4)
            .collect::<Vec<_>>();
        if urls.is_empty() {
            return Update::none();
        }
        self.image_loading = Some(self.image_generation.wrapping_add(1));
        self.image_generation = self.image_generation.wrapping_add(1);
        let generation = self.image_generation;
        let space = self.product.selected_space.clone();
        let auth = self.image_auth.clone();
        tracing::info!(target: "gchatui::diagnostics", generation, image_count = urls.len(), cached_count = self.images.len(), "image_batch");
        Update::none().with_command(Command::replace(
            CommandKey::new("rich-images"),
            async move {
                let images = crate::rich_content::load(urls, auth).await;
                Some(AppMessage::ImagesLoaded {
                    generation,
                    space,
                    images,
                })
            },
        ))
    }

    pub(super) fn handle_rich(&mut self, event: &Event) -> Option<Update<AppMessage>> {
        if self.viewer.is_some() {
            return Some(self.handle_viewer(event));
        }
        if self.auth_menu
            || self.help_visible
            || self.alias_picker.is_some()
            || self.alias_editor.is_some()
        {
            return None;
        }
        let action = event.key().and_then(|key| self.bindings.action_for(key));
        if self.focused_pane == FocusedPane::Conversation
            && action == Some(Action::Activate)
            && !self.rich_items.is_empty()
        {
            let index = self.rich_focus.unwrap_or_else(|| {
                self.rich_items
                    .iter()
                    .position(|(key, _)| {
                        self.conversation_view
                            .item_offset(key)
                            .is_some_and(|offset| {
                                offset >= self.conversation_view.scroll.vertical_offset()
                            })
                    })
                    .unwrap_or(0)
            });
            self.open_rich(index);
            return Some(Update::redraw());
        }
        if self.focused_pane == FocusedPane::Conversation
            && matches!(action, Some(Action::FocusNext | Action::FocusPrevious))
            && !self.rich_items.is_empty()
        {
            let count = self.rich_items.len();
            if self.rich_focus.is_some_and(|index| {
                (action == Some(Action::FocusNext) && index + 1 == count)
                    || (action == Some(Action::FocusPrevious) && index == 0)
            }) {
                self.rich_focus = None;
                return None;
            }
            let index = match (self.rich_focus, action) {
                (Some(index), Some(Action::FocusPrevious)) => (index + count - 1) % count,
                (Some(index), _) => (index + 1) % count,
                _ => 0,
            };
            self.rich_focus = Some(index);
            self.conversation_view.scroll_to_key(
                &self.rich_items[index].0,
                u64::from(conversation_content_area(self.conversation_pane.area).height),
            );
            return Some(Update::redraw());
        }
        None
    }

    pub(super) fn rich_target(
        &mut self,
        target: &str,
        event: &Event,
    ) -> Option<Update<AppMessage>> {
        let index = target.strip_prefix("rich-")?.parse::<usize>().ok()?;
        if index >= self.rich_items.len() {
            return None;
        }
        self.rich_focus = Some(index);
        if matches!(event, Event::Mouse(mouse) if mouse.kind == MouseEventKind::Up(MouseButton::Left))
        {
            self.open_rich(index);
        }
        Some(Update::redraw())
    }

    fn open_rich(&mut self, index: usize) {
        let mut content = self.rich_items[index].1.clone();
        if let Some(url) = &content.image_url
            && crate::chat_content::safe_url(url)
        {
            content
                .links
                .push(("Open image externally".into(), url.clone()));
        }
        self.viewer = Some(Viewer {
            links: vec![ButtonState::new(); content.links.len()],
            content,
            scroll: VirtualListState::new(0),
            close: ButtonState::new(),
            selected: 0,
        });
    }

    fn handle_viewer(&mut self, event: &Event) -> Update<AppMessage> {
        let action = event.key().and_then(|key| self.bindings.action_for(key));
        if action == Some(Action::Quit) {
            return self.apply_action(Action::Quit);
        }
        if action == Some(Action::Cancel) {
            self.viewer = None;
            return Update::redraw();
        }
        let area = self.viewer_area;
        let viewer = self.viewer.as_mut().expect("viewer active");
        if matches!(event, Event::Mouse(_))
            && Button::new("Close").handle_event(close_area(area), &mut viewer.close, event)
                == ButtonOutcome::Pressed
        {
            self.viewer = None;
            return Update::redraw();
        }
        match action {
            Some(Action::FocusNext) => {
                viewer.selected = (viewer.selected + 1) % (viewer.links.len() + 1);
            }
            Some(Action::FocusPrevious) => {
                viewer.selected = (viewer.selected + viewer.links.len()) % (viewer.links.len() + 1);
            }
            Some(Action::Activate) if viewer.selected == 0 => {
                self.viewer = None;
                return Update::redraw();
            }
            Some(Action::Activate) => {
                let url = viewer.content.links[viewer.selected - 1].1.clone();
                return open_link(url);
            }
            _ => {}
        }
        for (index, state) in viewer.links.iter_mut().enumerate() {
            if matches!(event, Event::Mouse(_))
                && Button::new("Open link").handle_event(link_area(area, index), state, event)
                    == ButtonOutcome::Pressed
            {
                return open_link(viewer.content.links[index].1.clone());
            }
        }
        let delta = match action {
            Some(Action::MoveDown) => 1,
            Some(Action::MoveUp) => -1,
            Some(Action::PageDown) => i64::from(area.height),
            Some(Action::PageUp) => -i64::from(area.height),
            _ => match event {
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollDown => 3,
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollUp => -3,
                _ => 0,
            },
        };
        let offset = viewer
            .scroll
            .scroll
            .vertical_offset()
            .saturating_add_signed(delta);
        viewer.scroll.scroll.set_vertical_offset(offset);
        Update::redraw()
    }
}

fn open_link(url: String) -> Update<AppMessage> {
    if !crate::chat_content::safe_url(&url) {
        return Update::none();
    }
    Update::none().with_command(Command::replace(
        CommandKey::new("open-rich-link"),
        async move {
            let _ = tokio::task::spawn_blocking(move || webbrowser::open(&url)).await;
            None
        },
    ))
}

fn close_area(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width.min(24), 1)
}
fn link_area(area: Rect, index: usize) -> Rect {
    Rect::new(
        area.x,
        area.y
            .saturating_add(1 + u16::try_from(index).unwrap_or(u16::MAX)),
        area.width,
        1,
    )
}

pub(super) fn paint_inline(app: &App, frame: &mut PaintCx<'_, '_>, area: Rect) {
    if app.viewer.is_some()
        || app.help_visible
        || app.auth_menu
        || app.alias_picker.is_some()
        || app.alias_editor.is_some()
    {
        return;
    }
    for (index, (key, content)) in app.rich_items.iter().enumerate() {
        let Some(offset) = app.conversation_view.item_offset(key) else {
            continue;
        };
        let Some(row) = offset
            .checked_sub(app.conversation_view.scroll.vertical_offset())
            .and_then(|v| u16::try_from(v).ok())
            .filter(|row| *row < area.height)
        else {
            continue;
        };
        let hit = Rect::new(
            area.x,
            area.y + row,
            area.width,
            if content.image_url.is_some() {
                7.min(area.height - row)
            } else {
                1
            },
        );
        frame.push_hit(
            HitRegion::new(HitId::new(format!("rich-{index}")), hit)
                .role(HitRole::Action)
                .hoverable(true)
                .focusable(true),
        );
        if app.rich_focus == Some(index) {
            frame.write_line(
                LocalRect::terminal(Rect::new(hit.x, hit.y, hit.width, 1)),
                &Line::from(format!("▶ {}", crate::rich_content::plain(&content.title))),
            );
        }
        if let Some(url) = &content.image_url {
            let destination = Rect::new(
                hit.x,
                hit.y + 1,
                hit.width.min(40),
                hit.height.saturating_sub(1),
            );
            paint_image(app, frame, url, destination, &format!("inline-{url}"));
        }
    }
}

fn paint_image(app: &App, frame: &mut PaintCx<'_, '_>, url: &str, area: Rect, key: &str) {
    if area.is_empty() {
        return;
    }
    if app.image_protocol.is_some()
        && let Some(Ok(payload)) = app.images.get(url)
    {
        let payload = if key == "expanded-image" {
            &payload.full
        } else {
            &payload.preview
        };
        let (width, height) = match payload {
            ImagePayload::Pixels { width, height, .. }
            | ImagePayload::Png { width, height, .. } => (*width, *height),
        };
        // Terminal cells are approximately twice as tall as they are wide.
        let columns = u32::from(area.width)
            .min(u32::from(area.height) * 2 * width / height.max(1))
            .max(1);
        let rows = (columns * height / width.max(1))
            .div_ceil(2)
            .max(1)
            .min(u32::from(area.height));
        frame.push_image(ImageContribution::Present(ImagePlacement {
            key: ImageKey::new(key),
            payload: payload.clone(),
            destination: Rect::new(
                area.x,
                area.y,
                u16::try_from(columns).unwrap_or(area.width),
                u16::try_from(rows).unwrap_or(area.height),
            ),
            clip: area,
            lifecycle: ImageLifecycle::Frame,
        }));
    } else {
        let label = if app.image_protocol.is_none() {
            "[Image: terminal has no image protocol]"
        } else if let Some(Err(error)) = app.images.get(url) {
            error.label()
        } else {
            "[Image preview pending]"
        };
        frame.write_line(LocalRect::terminal(area), &Line::from(label));
    }
}

pub(super) fn paint_viewer(app: &mut App, frame: &mut PaintCx<'_, '_>) {
    let Some(viewer) = app.viewer.as_ref() else {
        return;
    };
    let parent = raster_area(frame);
    let modal = ModalFrame::new(
        ModalSizing::fixed(
            Size::new(
                parent.width.saturating_sub(4),
                parent.height.saturating_sub(4),
            ),
            Insets::new(1, 1, 1, 1),
        ),
        ModalTheme::dark(ACCENT),
    )
    .title(crate::rich_content::plain(&viewer.content.title));
    let area = modal.content_area(parent);
    paint_component(
        frame,
        parent,
        &ModalFrameComponent::new("rich-viewer", modal, EmptyComponent),
    );
    app.viewer_area = area;
    let viewer = app.viewer.as_mut().expect("viewer active");
    let close_label = format!(
        "{} Close ({})",
        if viewer.selected == 0 { "▶" } else { " " },
        app.bindings.labels_for(Action::Cancel).join(" / ")
    );
    paint_component(
        frame,
        close_area(area),
        &ButtonComponent::new("rich-close", &close_label, &Cell::new(viewer.close))
            .styles(button_styles()),
    );
    for (index, ((label, _), state)) in viewer.content.links.iter().zip(&viewer.links).enumerate() {
        let label = format!(
            "{} {}",
            if viewer.selected == index + 1 {
                "▶"
            } else {
                "↗"
            },
            crate::rich_content::plain(label)
        );
        paint_component(
            frame,
            link_area(area, index),
            &ButtonComponent::new(format!("rich-link-{index}"), &label, &Cell::new(*state))
                .styles(button_styles()),
        );
    }
    let controls = u16::try_from(viewer.links.len() + 1)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let body = Rect::new(
        area.x,
        area.y.saturating_add(controls),
        area.width,
        area.height.saturating_sub(controls),
    );
    let diagnostic = viewer
        .content
        .image_url
        .as_ref()
        .and_then(|url| app.images.get(url))
        .and_then(|result| result.as_ref().err())
        .map(|error| error.details());
    if let Some(url) = viewer.content.image_url.clone()
        && diagnostic.is_none()
    {
        paint_image(app, frame, &url, body, "expanded-image");
    } else {
        let viewer = app.viewer.as_mut().expect("viewer active");
        let text = diagnostic.as_deref().unwrap_or(&viewer.content.text);
        let items = text
            .lines()
            .enumerate()
            .map(|(index, line)| TranscriptItem {
                key: index.to_string(),
                line: crate::rich_content::styled(line, Style::new().fg(TEXT).bg(SURFACE)),
            })
            .collect::<Vec<_>>();
        let list = transcript_list(&items);
        list.sync(
            u64::from(body.width),
            &mut viewer.scroll,
            &mut LayoutCx::new(),
        );
        frame.with_child(
            i32::from(body.x),
            i64::from(body.y),
            LocalRect::new(0, 0, body.width, body.height),
            |cx| {
                list.paint(Rect::new(0, 0, body.width, body.height), &viewer.scroll, cx);
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_image_revisit_presenter_measurement() {
        use bmux_tui_runtime::Presenter;
        let mut app = App::new(crate::keybind::KeybindingRegistry::default());
        let url = "https://example.com/synthetic.png";
        let pixels = |width, height| ImagePayload::Pixels {
            bytes: vec![127; width as usize * height as usize * 4],
            width,
            height,
            format: bmux_tui::image::ImagePixelFormat::Rgba8,
        };
        app.images.insert(
            url.into(),
            Ok(std::sync::Arc::new(crate::rich_content::DecodedImage {
                full: pixels(1024, 1024),
                preview: pixels(192, 192),
            })),
        );
        app.image_protocol = Some(bmux_image::ImageProtocol::KittyGraphics);
        let terminal =
            bmux_tui::terminal::Terminal::new(Vec::<u8>::new(), Rect::new(0, 0, 120, 40));
        let mut presenter = bmux_tui_runtime::ImageTerminalPresenter::new(
            terminal,
            |visible: &mut bool, cx: &mut PaintCx<'_, '_>| {
                if *visible {
                    paint_image(&app, cx, url, Rect::new(0, 0, 80, 30), "expanded-image");
                }
            },
            bmux_image::HostImageCapabilities {
                kitty_graphics: true,
                ..Default::default()
            },
            bmux_image::ImageConfig::default(),
        );
        for (stage, visible) in [
            ("first", true),
            ("unchanged", true),
            ("hidden", false),
            ("revisit", true),
        ] {
            let before = presenter.terminal().writer().len();
            let start = std::time::Instant::now();
            presenter.present(&mut { visible }).unwrap();
            let written = presenter.terminal().writer().len() - before;
            if stage == "unchanged" {
                assert!(written < 1024, "unchanged image must not retransmit pixels");
            }
            if stage == "revisit" {
                assert!(
                    written > 4 * 1024 * 1024,
                    "reproduction expects pinned bmux to retransmit the cached image"
                );
            }
            eprintln!(
                "synthetic_image stage={stage} elapsed_us={} output_bytes={}",
                start.elapsed().as_micros(),
                presenter.terminal().writer().len() - before
            );
        }
    }

    #[test]
    fn cached_preview_paints_without_pending_or_scheduling_network() {
        let mut app = App::new(crate::keybind::KeybindingRegistry::default());
        let url = "https://example.com/synthetic.png";
        let payload = ImagePayload::Pixels {
            bytes: vec![0; 16],
            width: 2,
            height: 2,
            format: bmux_tui::image::ImagePixelFormat::Rgba8,
        };
        app.images.insert(
            url.into(),
            Ok(std::sync::Arc::new(crate::rich_content::DecodedImage {
                full: payload.clone(),
                preview: payload,
            })),
        );
        app.image_protocol = Some(bmux_image::ImageProtocol::KittyGraphics);
        app.viewer = Some(Viewer {
            content: crate::model::RichContent {
                title: "Synthetic".into(),
                text: String::new(),
                image_url: Some(url.into()),
                links: Vec::new(),
            },
            scroll: VirtualListState::new(0),
            close: ButtonState::new(),
            links: Vec::new(),
            selected: 0,
        });
        let generation = app.image_generation;
        app.schedule_images();
        assert_eq!(app.image_generation, generation);
        assert!(app.image_loading.is_none());
        let mut buffer = bmux_tui::buffer::Buffer::empty(Rect::new(0, 0, 40, 8));
        {
            let mut frame = bmux_tui::frame::Frame::new(&mut buffer);
            paint_image(
                &app,
                &mut PaintCx::new(&mut frame),
                url,
                Rect::new(0, 0, 40, 8),
                "inline-test",
            );
        }
        let text = buffer
            .cells()
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>();
        assert!(!text.contains("pending"));
    }
}
