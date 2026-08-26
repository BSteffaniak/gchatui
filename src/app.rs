use std::io::{Stdout, stdout};
use std::sync::Arc;

use anyhow::Result;
#[cfg(test)]
use bmux_tui::buffer::Buffer;
use bmux_tui::crossterm::{CrosstermTerminalGuard, terminal_size};
use bmux_tui::event::{Event, MouseButton, MouseEventKind};
use bmux_tui::frame::Frame;
use bmux_tui::geometry::{Insets, Rect};
use bmux_tui::hit::HitMap;
use bmux_tui::interaction::InteractionRouter;
use bmux_tui::prelude::{Color, Line, Modifier, Span, Style};
use bmux_tui::terminal::Terminal;
use bmux_tui_components::button::{Button, ButtonOutcome, ButtonState, ButtonStyles};
use bmux_tui_components::key_hint_bar::{KeyHint, KeyHintBar, KeyHintBarPolicy, KeyHintBarStyles};
use bmux_tui_components::pane::{
    Pane, PaneMousePolicy, PaneOutcome, PanePolicy, PaneState, PaneStyles,
};
use bmux_tui_components::scroll_area::ScrollAreaScrollbarMode;
use bmux_tui_components::selectable_list::{
    SelectableList, SelectableListHighlightPolicy, SelectableListItem, SelectableListOutcome,
    SelectableListPolicy, SelectableListState, SelectableListStyles,
};
use bmux_tui_components::status_bar::{
    StatusBar, StatusBarPolicy, StatusBarStyles, StatusSegment, StatusSeverity,
};
use bmux_tui_components::text_view::{TextView, TextViewOutcome, TextViewState, TextViewStyles};
use bmux_tui_runtime::{
    Command, CommandKey, Lifecycle, Program, Runtime, RuntimeConfig, RuntimeEvent, TerminalInput,
    TerminalPresenter, Update,
};

use crate::chat::ChatClient;
use crate::credential::Secret;
use crate::keybind::{Action, KeybindingRegistry};
use crate::product::{Effect, Phase, ProductMessage, ProductState};

const CANVAS: Color = Color::Rgb(10, 14, 24);
const SURFACE: Color = Color::Rgb(16, 23, 38);
const SURFACE_RAISED: Color = Color::Rgb(24, 34, 53);
const BORDER: Color = Color::Rgb(52, 67, 91);
const TEXT: Color = Color::Rgb(226, 232, 240);
const MUTED: Color = Color::Rgb(125, 140, 165);
const ACCENT: Color = Color::Rgb(99, 179, 237);
const ACCENT_STRONG: Color = Color::Rgb(56, 189, 248);
const SELECTED_BG: Color = Color::Rgb(25, 64, 92);
const SUCCESS: Color = Color::Rgb(74, 222, 128);
const WARNING: Color = Color::Rgb(251, 191, 36);
const ERROR: Color = Color::Rgb(248, 113, 113);

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
    space_items: Arc<Vec<SelectableListItem>>,
    conversation_lines: Arc<Vec<Line>>,
    product: ProductState,
    chat: Arc<ChatClient>,
    access_token: Option<Arc<Secret>>,
    space_pane: PaneState,
    conversation_pane: PaneState,
    conversation_view: TextViewState,
    help_button: ButtonState,
    focused_pane: FocusedPane,
    follow_conversation_bottom: bool,
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
            space_items: Arc::new(synthetic_spaces()),
            conversation_lines: Arc::new(vec![Line::from("Select a space to read messages")]),
            product: ProductState::default(),
            chat: Arc::new(ChatClient::new()),
            access_token: None,
            space_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_view: TextViewState::new(),
            help_button: ButtonState::new(),
            focused_pane: FocusedPane::Spaces,
            follow_conversation_bottom: false,
            help_visible: false,
        }
    }

    fn rebuild_projections(&mut self) {
        self.space_items = Arc::new(project_spaces(&self.product));
        self.conversation_lines = Arc::new(project_conversation(&self.product));
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

    fn handle_direct_wheel(&mut self, event: &Event) -> Option<Update<AppMessage>> {
        let Event::Mouse(mouse) = event else {
            return None;
        };
        let delta = match mouse.kind {
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::ScrollDown => 3,
            MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
            | MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Move => return None,
        };
        if self.conversation_pane.area.contains(mouse.position) {
            let current =
                i32::try_from(self.conversation_view.vertical_scroll()).unwrap_or(i32::MAX);
            let next = usize::try_from(current.saturating_add(delta).max(0)).unwrap_or(usize::MAX);
            if next == self.conversation_view.vertical_scroll() {
                return Some(Update::none());
            }
            self.conversation_view.set_vertical_scroll(next);
            self.follow_conversation_bottom = false;
            return Some(Update::redraw());
        }
        if self.space_pane.area.contains(mouse.position) {
            let current = i32::try_from(self.spaces.vertical_scroll()).unwrap_or(i32::MAX);
            let next = usize::try_from(current.saturating_add(delta).max(0)).unwrap_or(usize::MAX);
            if next == self.spaces.vertical_scroll() {
                return Some(Update::none());
            }
            self.spaces.set_vertical_scroll(next);
            return Some(Update::redraw());
        }
        None
    }

    fn handle_space_event(&mut self, event: &Event) -> Option<Update<AppMessage>> {
        if let Event::Mouse(mouse) = event
            && !self.space_pane.area.contains(mouse.position)
        {
            return None;
        }
        if matches!(event, Event::Key(_)) && self.focused_pane != FocusedPane::Spaces {
            return None;
        }
        let spaces = Arc::clone(&self.space_items);
        let mut outcome = spaces_list(spaces.as_slice()).handle_event(
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

        if let Some(update) = self.handle_direct_wheel(&event) {
            return update;
        }
        if let Some(update) = self.handle_space_event(&event) {
            return update;
        }

        let conversation_lines = Arc::clone(&self.conversation_lines);
        let conversation_outcome = if let Event::Mouse(mouse) = event
            && !self.conversation_pane.area.contains(mouse.position)
        {
            TextViewOutcome::Ignored
        } else {
            TextView::new(conversation_lines.as_slice()).handle_event(
                conversation_content_area(self.conversation_pane.area),
                &mut self.conversation_view,
                &event,
            )
        };
        if matches!(conversation_outcome, TextViewOutcome::Scrolled { .. }) {
            self.follow_conversation_bottom = false;
        }
        if matches!(
            conversation_outcome,
            TextViewOutcome::Redraw | TextViewOutcome::Scrolled { .. }
        ) {
            return Update::reset();
        }

        let help = Button::new("Help");
        let help_outcome = help.handle_event(
            help_button_area(footer_area(self)),
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

    fn apply_scroll_action(&mut self, action: Action) {
        match self.focused_pane {
            FocusedPane::Spaces => {
                let area = space_list_area(self.space_pane.area);
                let amount = usize::from(area.height.max(1));
                let next = match action {
                    Action::PageDown => self.spaces.vertical_scroll().saturating_add(amount),
                    Action::PageUp => self.spaces.vertical_scroll().saturating_sub(amount),
                    Action::GoTop => 0,
                    Action::GoBottom => {
                        spaces_list(self.space_items.as_slice()).max_vertical_scroll(area)
                    }
                    _ => self.spaces.vertical_scroll(),
                };
                self.spaces.set_vertical_scroll(next);
            }
            FocusedPane::Conversation => {
                let lines = Arc::clone(&self.conversation_lines);
                let area = conversation_content_area(self.conversation_pane.area);
                let view = TextView::new(lines.as_slice());
                let amount = usize::from(area.height.max(1));
                let next = match action {
                    Action::PageDown => self
                        .conversation_view
                        .vertical_scroll()
                        .saturating_add(amount),
                    Action::PageUp => self
                        .conversation_view
                        .vertical_scroll()
                        .saturating_sub(amount),
                    Action::GoTop => 0,
                    Action::GoBottom => view.max_vertical_scroll(area),
                    _ => self.conversation_view.vertical_scroll(),
                };
                self.conversation_view.set_vertical_scroll(next);
                self.follow_conversation_bottom = matches!(action, Action::GoBottom);
            }
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
                self.conversation_view
                    .set_focused(self.focused_pane == FocusedPane::Conversation);
                Update::reset()
            }
            Action::MoveDown if self.focused_pane == FocusedPane::Conversation => {
                self.follow_conversation_bottom = false;
                self.conversation_view.set_vertical_scroll(
                    self.conversation_view.vertical_scroll().saturating_add(1),
                );
                Update::redraw()
            }
            Action::MoveUp if self.focused_pane == FocusedPane::Conversation => {
                self.follow_conversation_bottom = false;
                self.conversation_view.set_vertical_scroll(
                    self.conversation_view.vertical_scroll().saturating_sub(1),
                );
                Update::redraw()
            }
            Action::PageDown | Action::PageUp | Action::GoTop | Action::GoBottom => {
                self.apply_scroll_action(action);
                Update::redraw()
            }
            Action::MoveDown => {
                let item_count = self.space_items.len();
                let next = self
                    .spaces
                    .focused()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .min(item_count.saturating_sub(1));
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
                let messages_loaded = matches!(message, ProductMessage::MessagesLoaded { .. });
                self.product.update(message);
                self.rebuild_projections();
                if messages_loaded && matches!(self.product.phase, Phase::Ready | Phase::Empty) {
                    self.follow_conversation_bottom = true;
                }
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
                frame_interval: Some(std::time::Duration::from_millis(16)),
                max_active_commands: 4,
                max_queued_commands: 8,
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

fn project_spaces(product: &ProductState) -> Vec<SelectableListItem> {
    if product.spaces.is_empty() {
        return synthetic_spaces();
    }
    product
        .spaces
        .iter()
        .map(|space| {
            let icon = match space.kind {
                crate::model::SpaceKind::DirectMessage => "●",
                crate::model::SpaceKind::GroupChat => "◆",
                crate::model::SpaceKind::Space => "#",
                crate::model::SpaceKind::Unknown => "·",
            };
            let kind = match space.kind {
                crate::model::SpaceKind::DirectMessage => "DIRECT MESSAGE",
                crate::model::SpaceKind::GroupChat => "GROUP CHAT",
                crate::model::SpaceKind::Space => "SPACE",
                crate::model::SpaceKind::Unknown => "CONVERSATION",
            };
            SelectableListItem::multiline(
                space.id.0.clone(),
                vec![
                    Line::from_spans(vec![
                        Span::styled(format!("{icon} "), Style::new().fg(ACCENT_STRONG)),
                        Span::styled(
                            space.display_name.clone(),
                            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from_spans(vec![Span::styled(
                        format!("  {kind}"),
                        Style::new().fg(MUTED),
                    )]),
                ],
            )
        })
        .collect()
}

fn project_conversation(product: &ProductState) -> Vec<Line> {
    if product.messages.is_empty() {
        return vec![Line::from(match product.phase {
            Phase::LoadingMessages | Phase::Refreshing => "Loading messages…",
            Phase::Empty => "No messages",
            Phase::RecoverableError => "Unable to load messages. Refresh to retry.",
            Phase::Reauthentication => "Authorization expired. Sign in again.",
            _ => "Select a space to read messages",
        })];
    }
    let mut lines = Vec::new();
    let mut current_thread: Option<&str> = None;
    for message in &product.messages {
        let thread = message.thread_id.as_ref().map(|thread| thread.0.as_str());
        if thread != current_thread {
            if let Some(thread) = thread {
                lines.push(Line::from_spans(vec![
                    Span::styled("┌─ ", Style::new().fg(BORDER)),
                    Span::styled(
                        format!("THREAD {}", short_id(thread).to_uppercase()),
                        Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ─", Style::new().fg(BORDER)),
                ]));
            }
            current_thread = thread;
        }
        let sender = message
            .sender
            .as_ref()
            .map_or("Unknown sender", |sender| sender.display_name.as_str());
        let indent = if thread.is_some() { "  " } else { "" };
        lines.push(Line::from_spans(vec![
            Span::styled(
                format!("{indent}{sender}"),
                Style::new().fg(ACCENT_STRONG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", message.create_time), Style::new().fg(MUTED)),
        ]));
        lines.push(Line::from_spans(vec![Span::styled(
            format!("{indent}{}", message.text),
            Style::new().fg(TEXT),
        )]));
        if message.unsupported_content {
            lines.push(Line::from_spans(vec![Span::styled(
                format!("{indent}◇ Rich content is not available in the terminal"),
                Style::new().fg(WARNING),
            )]));
        }
        lines.push(Line::from(""));
    }
    lines
}

fn short_id(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

#[allow(clippy::too_many_lines)]
fn render(app: &mut App, frame: &mut Frame<'_>) {
    let area = frame.area();
    frame
        .buffer_mut()
        .fill(area, " ", Style::new().bg(CANVAS).fg(TEXT));
    if area.width < 20 || area.height < 6 {
        frame.buffer_mut().write_line(
            Rect::new(area.x, area.y, area.width, 1),
            &Line::from("Terminal is too small"),
        );
        return;
    }

    let header_height = 2;
    let footer_height = 2;
    let body_height = area.height.saturating_sub(header_height + footer_height);
    render_header(
        app,
        frame,
        Rect::new(area.x, area.y, area.width, header_height),
    );
    let body_y = area.y.saturating_add(header_height);
    let spaces_width = (area.width / 3).clamp(22, 38);
    let gap = u16::from(area.width >= 70);
    let spaces_area = Rect::new(area.x, body_y, spaces_width, body_height);
    let conversation_area = Rect::new(
        area.x.saturating_add(spaces_width).saturating_add(gap),
        body_y,
        area.width.saturating_sub(spaces_width).saturating_sub(gap),
        body_height,
    );
    app.space_pane.area = spaces_area;
    app.conversation_pane.area = conversation_area;

    let pane = interactive_pane();
    pane.clone()
        .title(Line::from_spans(vec![
            Span::styled(
                "  CONVERSATIONS",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", app.product.spaces.len()),
                Style::new().fg(MUTED),
            ),
        ]))
        .render_with_id("spaces-pane", &app.space_pane, frame);
    pane.title(Line::from_spans(vec![
        Span::styled(
            "  MESSAGES",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(selected_space_label(app), Style::new().fg(MUTED)),
    ]))
    .render_with_id("conversation-pane", &app.conversation_pane, frame);

    let spaces = Arc::clone(&app.space_items);
    spaces_list(spaces.as_slice()).render(space_list_area(spaces_area), &app.spaces, frame);

    let conversation = Arc::clone(&app.conversation_lines);
    let conversation_area = conversation_content_area(conversation_area);
    let conversation_view = TextView::new(conversation.as_slice()).styles(TextViewStyles {
        text: Style::new().fg(TEXT).bg(SURFACE),
        empty: Style::new().fg(MUTED).bg(SURFACE),
        background: Style::new().bg(SURFACE),
    });
    if app.follow_conversation_bottom {
        app.conversation_view
            .set_vertical_scroll(conversation_view.max_vertical_scroll(conversation_area));
        app.follow_conversation_bottom = false;
    }
    conversation_view.render(conversation_area, &app.conversation_view, frame);

    let footer_y = area
        .y
        .saturating_add(header_height)
        .saturating_add(body_height);
    let hint_labels = hints(app);
    let hints = hint_labels
        .iter()
        .map(|(key, label)| KeyHint::new(key, label))
        .collect::<Vec<_>>();
    KeyHintBar::new(&hints)
        .policy(KeyHintBarPolicy::compact())
        .styles(key_hint_styles())
        .render(Rect::new(area.x, footer_y, area.width, 1), frame);
    Button::new("  ? HELP  ")
        .styles(button_styles())
        .render_with_id(
            "help-button",
            help_button_area(Rect::new(area.x, footer_y, area.width, 1)),
            &app.help_button,
            frame,
        );
    if app.help_visible {
        render_help(app, frame, app.conversation_pane.area);
    }
    let status_text = status_text(app);
    let severity = status_severity(app.product.phase);
    let status = [StatusSegment::new(status_text).severity(severity)];
    let right = [StatusSegment::new("READ ONLY").severity(StatusSeverity::Muted)];
    StatusBar::new()
        .left(&status)
        .right(&right)
        .policy(StatusBarPolicy::compact().background(true))
        .styles(status_styles())
        .render(
            Rect::new(area.x, footer_y.saturating_add(1), area.width, 1),
            frame,
        );
}

fn render_header(app: &App, frame: &mut Frame<'_>, area: Rect) {
    frame
        .buffer_mut()
        .fill(area, " ", Style::new().bg(SURFACE_RAISED));
    frame.buffer_mut().write_line(
        Rect::new(
            area.x.saturating_add(1),
            area.y,
            area.width.saturating_sub(2),
            1,
        ),
        &Line::from_spans(vec![
            Span::styled(
                "gchat",
                Style::new().fg(ACCENT_STRONG).add_modifier(Modifier::BOLD),
            ),
            Span::styled("ui", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
            Span::styled("  /  terminal conversations", Style::new().fg(MUTED)),
        ]),
    );
    frame.buffer_mut().write_line(
        Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            1,
        ),
        &Line::from_spans(vec![
            Span::styled("● ", Style::new().fg(status_color(app.product.phase))),
            Span::styled(status_text(app), Style::new().fg(MUTED)),
        ]),
    );
}

fn selected_space_label(app: &App) -> String {
    app.product
        .selected_space
        .as_deref()
        .and_then(|id| app.product.spaces.iter().find(|space| space.id.0 == id))
        .map_or_else(String::new, |space| format!("  /  {}", space.display_name))
}

const fn status_text(app: &App) -> &'static str {
    if app.help_visible {
        return "Help overlay open";
    }
    match app.product.phase {
        Phase::MissingConfiguration => "Configure OAuth to connect",
        Phase::VaultSetup => "Creating secure local credentials",
        Phase::Unlock => "Unlocking local credentials",
        Phase::Login => "Waiting for Google authorization",
        Phase::LoadingSpaces => "Loading conversations…",
        Phase::Ready => "Connected",
        Phase::LoadingMessages => "Loading messages…",
        Phase::Refreshing => "Refreshing…",
        Phase::Empty => "Nothing here yet",
        Phase::RecoverableError => "Connection issue — press refresh to retry",
        Phase::Reauthentication => "Authorization expired — sign in again",
        Phase::FatalError => "gchatui encountered a fatal error",
    }
}

const fn status_severity(phase: Phase) -> StatusSeverity {
    match phase {
        Phase::Ready => StatusSeverity::Success,
        Phase::RecoverableError | Phase::Reauthentication => StatusSeverity::Warning,
        Phase::FatalError => StatusSeverity::Error,
        Phase::LoadingSpaces | Phase::LoadingMessages | Phase::Refreshing | Phase::Login => {
            StatusSeverity::Info
        }
        Phase::MissingConfiguration | Phase::VaultSetup | Phase::Unlock | Phase::Empty => {
            StatusSeverity::Muted
        }
    }
}

const fn status_color(phase: Phase) -> Color {
    match phase {
        Phase::Ready => SUCCESS,
        Phase::RecoverableError | Phase::Reauthentication => WARNING,
        Phase::FatalError => ERROR,
        Phase::LoadingSpaces | Phase::LoadingMessages | Phase::Refreshing | Phase::Login => ACCENT,
        Phase::MissingConfiguration | Phase::VaultSetup | Phase::Unlock | Phase::Empty => MUTED,
    }
}

const fn pane_styles() -> PaneStyles {
    PaneStyles {
        background: Some(Style::new().bg(SURFACE)),
        border: Style::new().fg(BORDER).bg(SURFACE),
        focused_border: Style::new()
            .fg(ACCENT_STRONG)
            .bg(SURFACE)
            .add_modifier(Modifier::BOLD),
    }
}

const fn list_styles() -> SelectableListStyles {
    SelectableListStyles {
        normal: Style::new().fg(TEXT).bg(SURFACE),
        focused: Style::new()
            .fg(TEXT)
            .bg(SURFACE_RAISED)
            .add_modifier(Modifier::BOLD),
        selected: Style::new()
            .fg(Color::BrightWhite)
            .bg(SELECTED_BG)
            .add_modifier(Modifier::BOLD),
        hovered: Style::new().fg(Color::BrightWhite).bg(SURFACE_RAISED),
        pressed: Style::new()
            .fg(Color::Black)
            .bg(ACCENT_STRONG)
            .add_modifier(Modifier::BOLD),
        disabled: Style::new()
            .fg(MUTED)
            .bg(SURFACE)
            .add_modifier(Modifier::DIM),
    }
}

const fn button_styles() -> ButtonStyles {
    ButtonStyles {
        normal: Style::new().fg(MUTED).bg(SURFACE_RAISED),
        focused: Style::new()
            .fg(Color::Black)
            .bg(ACCENT_STRONG)
            .add_modifier(Modifier::BOLD),
        hovered: Style::new().fg(Color::BrightWhite).bg(SELECTED_BG),
        pressed: Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD),
        disabled: Style::new()
            .fg(MUTED)
            .bg(SURFACE_RAISED)
            .add_modifier(Modifier::DIM),
    }
}

const fn key_hint_styles() -> KeyHintBarStyles {
    KeyHintBarStyles {
        key: Style::new()
            .fg(ACCENT_STRONG)
            .bg(SURFACE_RAISED)
            .add_modifier(Modifier::BOLD),
        label: Style::new().fg(MUTED).bg(SURFACE_RAISED),
        separator: Style::new().fg(BORDER).bg(SURFACE_RAISED),
        disabled: Style::new()
            .fg(MUTED)
            .bg(SURFACE_RAISED)
            .add_modifier(Modifier::DIM),
        background: Style::new().bg(SURFACE_RAISED),
    }
}

const fn status_styles() -> StatusBarStyles {
    StatusBarStyles {
        default: Style::new().fg(TEXT).bg(SURFACE_RAISED),
        muted: Style::new().fg(MUTED).bg(SURFACE_RAISED),
        info: Style::new().fg(ACCENT).bg(SURFACE_RAISED),
        success: Style::new().fg(SUCCESS).bg(SURFACE_RAISED),
        warning: Style::new()
            .fg(WARNING)
            .bg(SURFACE_RAISED)
            .add_modifier(Modifier::BOLD),
        error: Style::new()
            .fg(ERROR)
            .bg(SURFACE_RAISED)
            .add_modifier(Modifier::BOLD),
        separator: Style::new().fg(BORDER).bg(SURFACE_RAISED),
        background: Style::new().bg(SURFACE_RAISED),
    }
}

fn conversation_content_area(area: Rect) -> Rect {
    interactive_pane().inner_area(&PaneState::new(area))
}

fn spaces_list(items: &[SelectableListItem]) -> SelectableList<'_> {
    SelectableList::new(items)
        .policy(SelectableListPolicy {
            highlight: SelectableListHighlightPolicy::new("▌", true),
            ..SelectableListPolicy::interactive().scrollbar(ScrollAreaScrollbarMode::Gutter)
        })
        .styles(list_styles())
}

const fn footer_area(app: &App) -> Rect {
    Rect::new(
        app.space_pane.area.x,
        app.space_pane
            .area
            .y
            .saturating_add(app.space_pane.area.height),
        app.space_pane
            .area
            .width
            .saturating_add(app.conversation_pane.area.width)
            .saturating_add(1),
        1,
    )
}

const fn help_button_area(footer: Rect) -> Rect {
    Rect::new(
        footer.x.saturating_add(footer.width.saturating_sub(12)),
        footer.y,
        11,
        1,
    )
}

fn render_help(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let overlay = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2).min(10),
    );
    frame
        .buffer_mut()
        .fill(overlay, " ", Style::new().bg(SURFACE_RAISED));
    frame.buffer_mut().write_line(
        Rect::new(
            overlay.x.saturating_add(2),
            overlay.y,
            overlay.width.saturating_sub(4),
            1,
        ),
        &Line::from_spans(vec![Span::styled(
            "KEYBOARD SHORTCUTS",
            Style::new().fg(ACCENT_STRONG).add_modifier(Modifier::BOLD),
        )]),
    );
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
        if index.saturating_add(2) >= overlay.height {
            break;
        }
        let labels = app.bindings.labels_for(action).join(", ");
        if labels.is_empty() {
            continue;
        }
        frame.buffer_mut().write_line(
            Rect::new(
                overlay.x.saturating_add(2),
                overlay.y.saturating_add(2).saturating_add(index),
                overlay.width.saturating_sub(4),
                1,
            ),
            &Line::from_spans(vec![
                Span::styled(
                    format!("{labels:<16}"),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(action.label(), Style::new().fg(TEXT)),
            ]),
        );
    }
}

fn space_list_area(area: Rect) -> Rect {
    interactive_pane().inner_area(&PaneState::new(area))
}

fn interactive_pane() -> Pane<'static> {
    Pane::new()
        .padding(Insets::new(1, 1, 0, 1))
        .styles(pane_styles())
        .policy(PanePolicy {
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
    fn newly_loaded_messages_follow_latest_and_pane_scrolling_is_isolated() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.selected_space = Some("spaces/example".to_string());
        for index in 0..80 {
            app.product.messages.push(crate::model::Message {
                id: crate::model::MessageId(format!("messages/{index}")),
                thread_id: None,
                sender: None,
                text: format!("Synthetic message {index}"),
                create_time: format!("{index:03}"),
                unsupported_content: false,
            });
        }
        app.product.phase = Phase::Ready;
        app.rebuild_projections();
        app.follow_conversation_bottom = true;
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 100, 24));
        assert!(app.conversation_view.vertical_scroll() > 0);
        let left_before = app.spaces.vertical_scroll();
        let right_before = app.conversation_view.vertical_scroll();

        app.focused_pane = FocusedPane::Conversation;
        let up = "k".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(up.stroke()));
        assert_eq!(app.spaces.vertical_scroll(), left_before);
        assert!(app.conversation_view.vertical_scroll() < right_before);

        let right_point = Point::new(
            app.conversation_pane.area.x.saturating_add(2),
            app.conversation_pane.area.y.saturating_add(3),
        );
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::ScrollUp,
            right_point,
        )));
        assert_eq!(app.spaces.vertical_scroll(), left_before);
    }

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
        app.rebuild_projections();
        let rendered = app
            .conversation_lines
            .iter()
            .map(Line::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("THREAD example-thread".to_uppercase().as_str()));
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
        assert!(rendered.contains("CONVERSATIONS"));
        assert!(rendered.contains("Example Space"));
        assert!(rendered.contains("MESSAGES"));
        assert!(rendered.contains("HELP"));
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
        let footer_y = app
            .space_pane
            .area
            .y
            .saturating_add(app.space_pane.area.height);
        let area = help_button_area(footer_area(&app));
        assert_eq!(area.y, footer_y);
        let point = Point::new(area.x.saturating_add(2), area.y);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        assert!(app.help_button.interaction.pressed);
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
        let list_area = space_list_area(app.space_pane.area);
        let point = Point::new(list_area.x.saturating_add(2), list_area.y);
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
        let list_area = space_list_area(app.space_pane.area);
        let point = Point::new(list_area.x.saturating_add(2), list_area.y.saturating_add(4));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Up(MouseButton::Left),
            point,
        )));
        assert_eq!(app.spaces.selected(), Some(4));

        let down = "j".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(down.stroke()));
        assert_eq!(app.spaces.focused(), Some(5));
        assert_eq!(app.spaces.selected(), Some(5));
    }
}
