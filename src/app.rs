use std::io::{Stdout, stdout};
use std::sync::Arc;

use anyhow::Result;
#[cfg(test)]
use bmux_tui::buffer::Buffer;
use bmux_tui::crossterm::{CrosstermTerminalGuard, terminal_size};
use bmux_tui::event::{Event, MouseButton, MouseEventKind};
use bmux_tui::frame::Frame;
use bmux_tui::geometry::Rect;
use bmux_tui::hit::HitMap;
use bmux_tui::interaction::InteractionRouter;
use bmux_tui::prelude::Line;
use bmux_tui::terminal::Terminal;
use bmux_tui_components::button::{Button, ButtonOutcome, ButtonState};
use bmux_tui_components::key_hint_bar::{KeyHint, KeyHintBar};
use bmux_tui_components::pane::{Pane, PaneMousePolicy, PaneOutcome, PanePolicy, PaneState};
use bmux_tui_components::scroll_area::ScrollAreaScrollbarMode;
use bmux_tui_components::selectable_list::{
    SelectableList, SelectableListItem, SelectableListOutcome, SelectableListPolicy,
    SelectableListState,
};
use bmux_tui_components::status_bar::{StatusBar, StatusSegment};
use bmux_tui_components::text_view::{TextView, TextViewOutcome, TextViewState};
use bmux_tui_runtime::{
    Command, CommandKey, Lifecycle, Program, Runtime, RuntimeConfig, RuntimeEvent, TerminalInput,
    TerminalPresenter, Update,
};

use crate::chat::ChatClient;
use crate::credential::Secret;
use crate::keybind::{Action, KeybindingRegistry};
use crate::product::{Effect, Phase, ProductMessage, ProductState};

#[derive(Debug)]
pub enum AppMessage {
    Start,
    Product(ProductMessage),
    InputError(std::io::Error),
}

pub struct App {
    bindings: KeybindingRegistry,
    interactions: InteractionRouter,
    spaces: SelectableListState,
    product: ProductState,
    chat: Arc<ChatClient>,
    access_token: Option<Arc<Secret>>,
    space_pane: PaneState,
    conversation_pane: PaneState,
    conversation_view: TextViewState,
    help_button: ButtonState,
    focused_pane: FocusedPane,
    help_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedPane {
    Spaces,
    Conversation,
}

impl App {
    pub fn new(bindings: KeybindingRegistry) -> Self {
        Self {
            bindings,
            interactions: InteractionRouter::new(),
            spaces: SelectableListState::new(Some(0)),
            product: ProductState::default(),
            chat: Arc::new(ChatClient::new()),
            access_token: None,
            space_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_view: TextViewState::new(),
            help_button: ButtonState::new(),
            focused_pane: FocusedPane::Spaces,
            help_visible: false,
        }
    }

    const fn focus_spaces_pane(&mut self) {
        self.focused_pane = FocusedPane::Spaces;
        self.space_pane.interaction.focused = true;
        self.conversation_pane.interaction.focused = false;
    }

    fn command_for_effect(&self, effect: Effect) -> Option<Command<AppMessage>> {
        let token = self.access_token.clone()?;
        let chat = Arc::clone(&self.chat);
        match effect {
            Effect::LoadSpaces { request_id } => {
                Some(Command::replace(CommandKey::new("spaces"), async move {
                    let result = chat
                        .list_spaces(token.as_ref(), 100, None)
                        .await
                        .map(|page| (page.items, page.next_page_token));
                    Some(AppMessage::Product(ProductMessage::SpacesLoaded {
                        request_id,
                        result,
                    }))
                }))
            }
            Effect::LoadMessages {
                request_id,
                space_name,
                page_token,
            } => Some(Command::replace(CommandKey::new("messages"), async move {
                let result = chat
                    .list_messages(
                        token.as_ref(),
                        &crate::model::SpaceId(space_name.clone()),
                        100,
                        page_token.as_ref(),
                    )
                    .await
                    .map(|page| (page.items, page.next_page_token));
                Some(AppMessage::Product(ProductMessage::MessagesLoaded {
                    request_id,
                    space_name,
                    result,
                }))
            })),
        }
    }

    fn handle_space_event(&mut self, event: &Event) -> Option<Update<AppMessage>> {
        let spaces = displayed_spaces(self);
        let mut outcome = spaces_list(&spaces).handle_event(
            space_list_area(self.space_pane.area),
            &mut self.spaces,
            event,
        );
        if let Event::Mouse(mouse) = event
            && matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left))
            && matches!(outcome, SelectableListOutcome::Ignored)
            && space_list_area(self.space_pane.area).contains(mouse.position)
            && let Some(index) = self.spaces.selected()
        {
            outcome = SelectableListOutcome::Selected(index);
        }
        match outcome {
            SelectableListOutcome::Selected(index) => {
                self.focus_spaces_pane();
                let effect = self
                    .product
                    .spaces
                    .get(index)
                    .map(|space| space.id.0.clone())
                    .map(|space_name| self.product.select_space(space_name));
                Some(
                    effect
                        .and_then(|effect| self.command_for_effect(effect))
                        .map_or_else(Update::reset, |command| {
                            Update::reset().with_command(command)
                        }),
                )
            }
            SelectableListOutcome::Focused(_) | SelectableListOutcome::Redraw => {
                self.focus_spaces_pane();
                Some(Update::reset())
            }
            SelectableListOutcome::Ignored => None,
        }
    }

    fn update_terminal(&mut self, event: Event) -> Update<AppMessage> {
        if let Event::Key(stroke) = event
            && let Some(action) = self.bindings.action_for(stroke)
        {
            return self.apply_action(action);
        }

        if let Some(update) = self.handle_space_event(&event) {
            return update;
        }

        let conversation_lines = conversation_lines(self);
        let conversation_outcome = TextView::new(&conversation_lines).handle_event(
            conversation_content_area(self.conversation_pane.area),
            &mut self.conversation_view,
            &event,
        );
        if matches!(
            conversation_outcome,
            TextViewOutcome::Redraw | TextViewOutcome::Scrolled { .. }
        ) {
            return Update::reset();
        }

        let help = Button::new("Help");
        let help_outcome = help.handle_event(
            help_button_area(self.space_pane.area),
            &mut self.help_button,
            &event,
        );
        if matches!(help_outcome, ButtonOutcome::Pressed) {
            self.help_visible = !self.help_visible;
            return Update::reset();
        }
        if help_outcome.is_handled() {
            return if help_outcome.needs_redraw() {
                Update::reset()
            } else {
                Update::none()
            };
        }

        let pane = interactive_pane();
        let pane_outcome = pane.handle_event(&mut self.space_pane, &event);
        let conversation_outcome = pane.handle_event(&mut self.conversation_pane, &event);
        if matches!(pane_outcome, PaneOutcome::FocusRequested) {
            self.focused_pane = FocusedPane::Spaces;
            self.space_pane.interaction.focused = true;
            self.conversation_pane.interaction.focused = false;
        }
        if matches!(conversation_outcome, PaneOutcome::FocusRequested) {
            self.focused_pane = FocusedPane::Conversation;
            self.space_pane.interaction.focused = false;
            self.conversation_pane.interaction.focused = true;
        }
        if !matches!(pane_outcome, PaneOutcome::Ignored)
            || !matches!(conversation_outcome, PaneOutcome::Ignored)
        {
            return Update::reset();
        }

        let routed = self.interactions.route(event);
        if routed.traversal_consumed
            || routed.focus_changed.is_some()
            || routed.hover_left.is_some()
            || routed.hover_entered.is_some()
        {
            Update::reset()
        } else {
            Update::none()
        }
    }

    fn apply_action(&mut self, action: Action) -> Update<AppMessage> {
        match action {
            Action::Quit => Update {
                lifecycle: Lifecycle::Exit,
                ..Update::none()
            },
            Action::Refresh => self
                .product
                .refresh()
                .and_then(|effect| self.command_for_effect(effect))
                .map_or_else(Update::reset, |command| {
                    Update::reset().with_command(command)
                }),
            Action::Activate => {
                let effect = if let Some(index) = self.spaces.selected()
                    && let Some(space) = self.product.spaces.get(index)
                {
                    Some(self.product.select_space(space.id.0.clone()))
                } else {
                    None
                };
                effect
                    .and_then(|effect| self.command_for_effect(effect))
                    .map_or_else(Update::reset, |command| {
                        Update::reset().with_command(command)
                    })
            }
            Action::Help => {
                self.help_visible = !self.help_visible;
                Update::reset()
            }
            Action::FocusNext | Action::FocusPrevious => {
                self.focused_pane = match self.focused_pane {
                    FocusedPane::Spaces => FocusedPane::Conversation,
                    FocusedPane::Conversation => FocusedPane::Spaces,
                };
                self.space_pane.interaction.focused = self.focused_pane == FocusedPane::Spaces;
                self.conversation_pane.interaction.focused =
                    self.focused_pane == FocusedPane::Conversation;
                Update::reset()
            }
            Action::MoveDown => {
                let next = self
                    .spaces
                    .focused()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .min(synthetic_spaces().len().saturating_sub(1));
                self.spaces.set_focused(Some(next));
                self.spaces.set_selected(Some(next));
                Update::reset()
            }
            Action::MoveUp => {
                let previous = self.spaces.focused().unwrap_or(0).saturating_sub(1);
                self.spaces.set_focused(Some(previous));
                self.spaces.set_selected(Some(previous));
                Update::reset()
            }
            _ => Update::none(),
        }
    }
}

impl Program for App {
    type Message = AppMessage;
    type Error = std::io::Error;

    fn update(
        &mut self,
        event: RuntimeEvent<Self::Message>,
    ) -> Result<Update<Self::Message>, Self::Error> {
        match event {
            RuntimeEvent::Terminal(event) => Ok(self.update_terminal(event)),
            RuntimeEvent::Message(AppMessage::Start) => Ok(startup_update(self)),
            RuntimeEvent::Message(AppMessage::Product(message)) => {
                self.product.update(message);
                sync_space_selection(self);
                Ok(Update::reset())
            }
            RuntimeEvent::Message(AppMessage::InputError(error)) => Err(error),
            RuntimeEvent::Timer(_) => Ok(Update::none()),
        }
    }
}

pub async fn run(bindings: KeybindingRegistry, access_token: Option<Secret>) -> Result<()> {
    let mut guard = CrosstermTerminalGuard::enter(stdout())?;
    let result = {
        let writer = guard.writer_mut().expect("guard should own stdout");
        let size = terminal_size()?;
        let terminal = Terminal::new(writer, Rect::new(0, 0, size.width, size.height));
        let presenter = TerminalPresenter::with_commit(
            terminal,
            render,
            |app: &mut App, hits: &HitMap, _focus: &bmux_tui::focus::FocusTrap| {
                app.interactions.commit_scene(hits.clone(), None);
            },
        );
        let mut app = App::new(bindings);
        let startup = access_token.map(|token| {
            app.access_token = Some(Arc::new(token));
        });
        let (runtime, handle) = Runtime::new(
            app,
            presenter,
            RuntimeConfig {
                frame_interval: None,
                ..RuntimeConfig::default()
            },
        );
        let _input = TerminalInput::start::<App>(handle.clone(), AppMessage::InputError);
        if startup.is_some() {
            let _ = handle.send(AppMessage::Start).await;
        }
        match runtime.run().await {
            Ok(_) => Ok(()),
            Err(
                bmux_tui_runtime::RuntimeError::Program { error, .. }
                | bmux_tui_runtime::RuntimeError::Presenter { error, .. },
            ) => Err(error),
        }
    };
    let _stdout: Stdout = guard.leave()?;
    result.map_err(Into::into)
}

fn startup_update(app: &mut App) -> Update<AppMessage> {
    // Startup has already moved state to LoadingSpaces. Recreate the command from
    // the active request by allocating a replacement request; the old request has
    // never been scheduled and is therefore intentionally superseded.
    let effect = app.product.load_spaces();
    app.command_for_effect(effect)
        .map_or_else(Update::reset, |command| {
            Update::reset().with_command(command)
        })
}

fn sync_space_selection(app: &mut App) {
    if app.product.spaces.is_empty() {
        app.spaces.set_selected(None);
        app.spaces.set_focused(None);
        return;
    }
    let selected = app
        .spaces
        .selected()
        .unwrap_or(0)
        .min(app.product.spaces.len() - 1);
    app.spaces.set_selected(Some(selected));
    app.spaces.set_focused(Some(selected));
}

fn displayed_spaces(app: &App) -> Vec<SelectableListItem> {
    if app.product.spaces.is_empty() {
        return synthetic_spaces();
    }
    app.product
        .spaces
        .iter()
        .map(|space| SelectableListItem::new(space.id.0.clone(), space.display_name.clone()))
        .collect()
}

fn conversation_lines(app: &App) -> Vec<Line> {
    if app.product.messages.is_empty() {
        return vec![Line::from(match app.product.phase {
            Phase::LoadingMessages | Phase::Refreshing => "Loading messages…",
            Phase::Empty => "No messages",
            Phase::RecoverableError => "Unable to load messages. Refresh to retry.",
            Phase::Reauthentication => "Authorization expired. Sign in again.",
            _ => "Select a space to read messages",
        })];
    }
    let mut lines = Vec::new();
    let mut current_thread: Option<&str> = None;
    for message in &app.product.messages {
        let thread = message.thread_id.as_ref().map(|thread| thread.0.as_str());
        if thread != current_thread {
            if let Some(thread) = thread {
                lines.push(Line::from(format!("Thread {}", short_id(thread))));
            }
            current_thread = thread;
        }
        let sender = message
            .sender
            .as_ref()
            .map_or("Unknown sender", |sender| sender.display_name.as_str());
        let indent = if thread.is_some() { "  " } else { "" };
        lines.push(Line::from(format!(
            "{indent}{sender}  {}",
            message.create_time
        )));
        lines.push(Line::from(format!("{indent}{}", message.text)));
        if message.unsupported_content {
            lines.push(Line::from(format!("{indent}[Unsupported rich content]")));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn short_id(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn render(app: &mut App, frame: &mut Frame<'_>) {
    let area = frame.area();
    if area.width < 20 || area.height < 6 {
        frame.buffer_mut().write_line(
            Rect::new(area.x, area.y, area.width, 1),
            &Line::from("Terminal is too small"),
        );
        return;
    }

    let body_height = area.height.saturating_sub(2);
    let spaces_width = (area.width / 3).clamp(18, 32);
    let spaces_area = Rect::new(area.x, area.y, spaces_width, body_height);
    let conversation_area = Rect::new(
        area.x.saturating_add(spaces_width),
        area.y,
        area.width.saturating_sub(spaces_width),
        body_height,
    );
    app.space_pane.area = spaces_area;
    app.conversation_pane.area = conversation_area;

    let pane = interactive_pane();
    pane.clone()
        .title("Spaces")
        .render_with_id("spaces-pane", &app.space_pane, frame);
    pane.title("Conversation")
        .render_with_id("conversation-pane", &app.conversation_pane, frame);

    let spaces = displayed_spaces(app);
    spaces_list(&spaces).render(space_list_area(spaces_area), &app.spaces, frame);

    let conversation = conversation_lines(app);
    TextView::new(&conversation).render(
        conversation_content_area(conversation_area),
        &app.conversation_view,
        frame,
    );

    let hint_labels = hints(app);
    let hints = hint_labels
        .iter()
        .map(|(key, label)| KeyHint::new(key, label))
        .collect::<Vec<_>>();
    KeyHintBar::new(&hints).render(
        Rect::new(area.x, area.y.saturating_add(body_height), area.width, 1),
        frame,
    );
    Button::new("Help").render_with_id(
        "help-button",
        help_button_area(spaces_area),
        &app.help_button,
        frame,
    );
    if app.help_visible {
        render_help(app, frame, conversation_area);
    }
    let status = [StatusSegment::new(if app.help_visible {
        "Help is visible"
    } else {
        "Read-only prototype"
    })];
    StatusBar::new().left(&status).render(
        Rect::new(
            area.x,
            area.y.saturating_add(body_height).saturating_add(1),
            area.width,
            1,
        ),
        frame,
    );
}

const fn conversation_content_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn spaces_list(items: &[SelectableListItem]) -> SelectableList<'_> {
    SelectableList::new(items)
        .policy(SelectableListPolicy::interactive().scrollbar(ScrollAreaScrollbarMode::Gutter))
}

const fn help_button_area(spaces: Rect) -> Rect {
    Rect::new(
        spaces.x.saturating_add(spaces.width.saturating_sub(7)),
        spaces.y,
        6,
        1,
    )
}

fn render_help(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let rows = [
        Action::FocusNext,
        Action::MoveDown,
        Action::Activate,
        Action::Refresh,
        Action::Help,
        Action::Quit,
    ];
    for (index, action) in rows.into_iter().enumerate() {
        let Ok(index) = u16::try_from(index) else {
            break;
        };
        if index.saturating_add(2) >= area.height {
            break;
        }
        let labels = app.bindings.labels_for(action).join(", ");
        if labels.is_empty() {
            continue;
        }
        frame.buffer_mut().write_line(
            Rect::new(
                area.x.saturating_add(2),
                area.y.saturating_add(2).saturating_add(index),
                area.width.saturating_sub(4),
                1,
            ),
            &Line::from(format!("{labels:<16} {}", action.label())),
        );
    }
}

const fn space_list_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn interactive_pane() -> Pane<'static> {
    Pane::new().policy(PanePolicy {
        mouse: PaneMousePolicy {
            enabled: true,
            click_to_focus: true,
            title_bar_drag: false,
            scroll_wheel: false,
            resize_handles: bmux_tui_components::pane::ResizeHandles::NONE,
        },
        ..PanePolicy::default()
    })
}

#[cfg(test)]
fn render_to_buffer(app: &mut App, area: Rect) -> Buffer {
    let mut buffer = Buffer::empty(area);
    let mut frame = Frame::new(&mut buffer);
    render(app, &mut frame);
    buffer
}

fn hints(app: &App) -> Vec<(String, &'static str)> {
    [Action::Help, Action::Refresh, Action::Quit]
        .into_iter()
        .filter_map(|action| {
            app.bindings
                .labels_for(action)
                .first()
                .cloned()
                .map(|label| (label, action.label()))
        })
        .collect()
}

fn synthetic_spaces() -> Vec<SelectableListItem> {
    (1..=30)
        .map(|index| {
            let label = match index {
                1 => "Example Space".to_string(),
                2 => "Project Discussion".to_string(),
                3 => "Release Planning".to_string(),
                4 => "Example User".to_string(),
                _ => format!("Example Space {index}"),
            };
            SelectableListItem::new(format!("space-{index}"), label)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use bmux_tui::event::{MouseButton, MouseEvent, MouseEventKind};
    use bmux_tui::geometry::Point;

    #[test]
    fn conversation_groups_thread_replies() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.messages = vec![crate::model::Message {
            id: crate::model::MessageId("messages/example".to_string()),
            thread_id: Some(crate::model::ThreadId("threads/example-thread".to_string())),
            sender: Some(crate::model::Sender {
                display_name: "Example User".to_string(),
            }),
            text: "Synthetic reply".to_string(),
            create_time: "10:42".to_string(),
            unsupported_content: false,
        }];
        let rendered = conversation_lines(&app)
            .iter()
            .map(Line::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Thread example-thread"));
        assert!(rendered.contains("  Example User"));
        assert!(rendered.contains("  Synthetic reply"));
    }

    #[test]
    fn hint_text_comes_from_registry() {
        let app = App::new(KeybindingRegistry::default());
        let labels = hints(&app);
        assert!(
            labels
                .iter()
                .any(|(key, label)| key == "q" && *label == "Quit")
        );
    }

    #[test]
    fn synthetic_spaces_are_public_safe() {
        assert_eq!(synthetic_spaces().len(), 30);
    }

    #[test]
    fn frame_contains_component_content_and_registry_hints() {
        let mut app = App::new(KeybindingRegistry::default());
        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        let rendered = (0..20)
            .filter_map(|row| buffer.row_symbols(row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Spaces"));
        assert!(rendered.contains("Example Space"));
        assert!(rendered.contains("Conversation"));
        assert!(rendered.contains("Help"));
        assert!(rendered.contains("Quit"));
    }

    #[test]
    fn focus_traversal_and_pointer_focus_share_state() {
        let mut app = App::new(KeybindingRegistry::default());
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        assert_eq!(app.focused_pane, FocusedPane::Spaces);

        let tab = "Tab".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(tab.stroke()));
        assert_eq!(app.focused_pane, FocusedPane::Conversation);

        let point = Point::new(2, 2);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        assert_eq!(app.focused_pane, FocusedPane::Spaces);
    }

    #[test]
    fn help_button_and_help_rows_use_active_registry() {
        let mut app = App::new(KeybindingRegistry::default());
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        let area = help_button_area(app.space_pane.area);
        let point = Point::new(area.x, area.y);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Up(MouseButton::Left),
            point,
        )));
        assert!(app.help_visible);
        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        let rendered = (0..20)
            .filter_map(|row| buffer.row_symbols(row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Refresh"));
        assert!(rendered.contains("Ctrl+c"));
    }

    #[test]
    fn mouse_wheel_scrolls_long_space_list() {
        let mut app = App::new(KeybindingRegistry::default());
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 12));
        assert_eq!(app.spaces.vertical_scroll(), 0);
        let point = Point::new(3, 4);
        for _ in 0..4 {
            let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
                MouseEventKind::ScrollDown,
                point,
            )));
        }
        assert!(app.spaces.vertical_scroll() > 0);
    }

    #[test]
    fn selecting_real_space_opens_conversation_for_mouse_and_keyboard() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.spaces = vec![crate::model::Space {
            id: crate::model::SpaceId("spaces/example".to_string()),
            display_name: "Example Space".to_string(),
            kind: crate::model::SpaceKind::Space,
        }];
        app.product.phase = Phase::Ready;
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        let point = Point::new(3, 1);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Up(MouseButton::Left),
            point,
        )));
        assert_eq!(
            app.product.selected_space.as_deref(),
            Some("spaces/example")
        );
        assert_eq!(app.product.phase, Phase::LoadingMessages);

        app.product.phase = Phase::Ready;
        let enter = "Enter".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(enter.stroke()));
        assert_eq!(
            app.product.selected_space.as_deref(),
            Some("spaces/example")
        );
        assert_eq!(app.product.phase, Phase::LoadingMessages);
    }

    #[test]
    fn mouse_click_selects_space_and_keyboard_continues_from_it() {
        let mut app = App::new(KeybindingRegistry::default());
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        let point = Point::new(2, 3);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Up(MouseButton::Left),
            point,
        )));
        assert_eq!(app.spaces.selected(), Some(2));

        let down = "j".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(down.stroke()));
        assert_eq!(app.spaces.focused(), Some(3));
        assert_eq!(app.spaces.selected(), Some(3));
    }
}
