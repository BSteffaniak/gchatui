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
    pub(super) fn load_images(&mut self) -> Command<AppMessage> {
        self.image_generation = self.image_generation.wrapping_add(1);
        let generation = self.image_generation;
        let allowed = self
            .rich_items
            .iter()
            .filter_map(|(_, content)| content.image_url.clone())
            .collect::<std::collections::BTreeSet<_>>();
        self.images
            .retain(|url, _| allowed.iter().take(16).any(|allowed| allowed == url));
        let space = self.product.selected_space.clone();
        tracing::info!(target: "gchatui::diagnostics", generation, image_count = allowed.len(), limited_count = allowed.len().saturating_sub(16), "image_batch");
        for url in allowed.iter().skip(16) {
            self.images
                .insert(url.clone(), Err(crate::rich_content::ImageError::Limit));
        }
        let cached = self
            .images
            .iter()
            .filter(|(_, result)| result.is_ok())
            .map(|(url, image)| (url.clone(), image.clone()))
            .collect::<crate::rich_content::Images>();
        let urls = self
            .rich_items
            .iter()
            .filter_map(|(_, content)| content.image_url.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(16)
            .filter(|url| !cached.contains_key(url))
            .collect();
        let auth = self.image_auth.clone();
        Command::replace(CommandKey::new("rich-images"), async move {
            let mut images = cached;
            images.extend(crate::rich_content::load(urls, auth).await);
            for url in allowed.into_iter().skip(16) {
                images.insert(url, Err(crate::rich_content::ImageError::Limit));
            }
            Some(AppMessage::ImagesLoaded {
                generation,
                space,
                images,
            })
        })
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
            paint_image(app, frame, url, destination, &format!("inline-{index}"));
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
        let (width, height) = match payload.as_ref() {
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
            payload: payload.as_ref().clone(),
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
