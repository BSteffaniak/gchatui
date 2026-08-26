use std::io::{Stdout, stdout};
use std::sync::Arc;

use anyhow::Result;
#[cfg(test)]
use bmux_tui::buffer::Buffer;
use bmux_tui::crossterm::{CrosstermTerminalGuard, terminal_size};
use bmux_tui::event::{Event, MouseButton, MouseEventKind};
use bmux_tui::frame::Frame;
use bmux_tui::geometry::{Insets, Point, Rect};
use bmux_tui::hit::{HitId, HitMap, HitRegion, HitRole};
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
use bmux_tui_components::text_input::TextInputState;
use bmux_tui_components::text_input_box::{TextInputBox, TextInputBoxOutcome, TextInputBoxPolicy};
use bmux_tui_components::text_view::{TextView, TextViewOutcome, TextViewState, TextViewStyles};
use bmux_tui_runtime::{
    Command, CommandKey, Lifecycle, Program, Runtime, RuntimeConfig, RuntimeEvent, TerminalInput,
    TerminalPresenter, Update,
};

use crate::chat::ChatClient;
use crate::credential::Secret;
use crate::keybind::{Action, KeybindingRegistry};
use crate::people::PeopleClient;
use crate::product::{Effect, Phase, ProductMessage, ProductState};
use crate::sender_alias::SenderAliases;
use crate::transcript_projection::{ThreadActivityLink, TranscriptColors, TranscriptProjection};

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
    MessagesWithIdentity {
        message: ProductMessage,
        current_user: Option<(std::collections::BTreeSet<String>, Option<String>)>,
    },
    InputError(std::io::Error),
}

pub struct App {
    bindings: KeybindingRegistry,
    interactions: InteractionRouter,
    spaces: SelectableListState,
    space_items: Arc<Vec<SelectableListItem>>,
    conversation_lines: Arc<Vec<Line>>,
    thread_activity_links: Arc<Vec<ThreadActivityLink>>,
    focused_thread_activity: Option<usize>,
    product: ProductState,
    chat: Arc<ChatClient>,
    people: Arc<PeopleClient>,
    aliases: Option<Arc<SenderAliases>>,
    access_token: Option<Arc<Secret>>,
    space_pane: PaneState,
    conversation_pane: PaneState,
    conversation_view: TextViewState,
    help_button: ButtonState,
    alias_picker: Option<AliasPicker>,
    alias_editor: Option<AliasEditor>,
    focused_pane: FocusedPane,
    follow_conversation_bottom: bool,
    help_visible: bool,
}

struct AliasPicker {
    senders: Vec<AliasSender>,
    items: Vec<SelectableListItem>,
    state: SelectableListState,
}

struct AliasSender {
    resource_name: String,
    label: String,
}

struct AliasEditor {
    resource_name: String,
    input: TextInputState,
    error: Option<String>,
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
            space_items: Arc::new(Vec::new()),
            conversation_lines: Arc::new(vec![Line::from("Select a space to read messages")]),
            thread_activity_links: Arc::new(Vec::new()),
            focused_thread_activity: None,
            product: ProductState::default(),
            chat: Arc::new(ChatClient::new()),
            people: Arc::new(PeopleClient::new()),
            aliases: None,
            access_token: None,
            space_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_view: TextViewState::new(),
            help_button: ButtonState::new(),
            alias_picker: None,
            alias_editor: None,
            focused_pane: FocusedPane::Spaces,
            follow_conversation_bottom: false,
            help_visible: false,
        }
    }

    fn rebuild_projections(&mut self) {
        self.space_items = Arc::new(project_spaces(&self.product));
        let projection = project_conversation(&self.product);
        self.conversation_lines = Arc::new(projection.lines);
        self.thread_activity_links = Arc::new(projection.links);
        self.focused_thread_activity = None;
    }

    const fn focus_spaces_pane(&mut self) {
        self.focused_pane = FocusedPane::Spaces;
        self.space_pane.interaction.focused = true;
        self.conversation_pane.interaction.focused = false;
    }

    fn command_for_effect(&self, effect: Effect) -> Option<Command<AppMessage>> {
        let token = self.access_token.clone()?;
        let chat = Arc::clone(&self.chat);
        let people = Arc::clone(&self.people);
        let aliases = self.aliases.clone();
        match effect {
            Effect::LoadSpaces { request_id } => {
                Some(Command::replace(CommandKey::new("spaces"), async move {
                    let result = match chat.list_spaces(token.as_ref(), 100, None).await {
                        Ok(mut page) => {
                            chat.infer_direct_message_titles(
                                token.as_ref(),
                                &mut page.items,
                                &people,
                                aliases.as_deref(),
                            )
                            .await;
                            Ok((page.items, page.next_page_token))
                        }
                        Err(error) => Err(error),
                    };
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
                let page = match chat
                    .list_messages(
                        token.as_ref(),
                        &crate::model::SpaceId(space_name.clone()),
                        100,
                        page_token.as_ref(),
                    )
                    .await
                {
                    Ok(page) => page,
                    Err(error) => {
                        return Some(AppMessage::Product(ProductMessage::MessagesLoaded {
                            request_id,
                            space_name,
                            result: Err(error),
                        }));
                    }
                };
                let mut messages = page.items;
                let _ = people
                    .resolve_message_senders(token.as_ref(), &mut messages)
                    .await;
                let current_user = people.current_user(token.as_ref()).await.ok();
                Some(AppMessage::MessagesWithIdentity {
                    message: ProductMessage::MessagesLoaded {
                        request_id,
                        space_name,
                        result: Ok((messages, page.next_page_token)),
                    },
                    current_user,
                })
            })),
        }
    }

    fn handle_alias_picker(&mut self, event: &Event) -> Update<AppMessage> {
        let area = alias_picker_content_area(self.conversation_pane.area);
        let picker = self.alias_picker.as_mut().expect("alias picker checked");
        match spaces_list(picker.items.as_slice()).handle_event(area, &mut picker.state, event) {
            SelectableListOutcome::Selected(index) => {
                if let Some(sender) = picker.senders.get(index) {
                    let initial = self
                        .aliases
                        .as_ref()
                        .and_then(|aliases| aliases.get(&sender.resource_name))
                        .unwrap_or_default();
                    self.alias_editor = Some(AliasEditor {
                        resource_name: sender.resource_name.clone(),
                        input: TextInputState::new(bmux_text_edit::TextEditBuffer::from_text(
                            initial,
                        )),
                        error: None,
                    });
                    self.alias_picker = None;
                }
                Update::reset()
            }
            SelectableListOutcome::Focused(_) | SelectableListOutcome::Redraw => Update::redraw(),
            SelectableListOutcome::Ignored => Update::none(),
        }
    }

    fn handle_alias_editor(&mut self, event: &Event) -> Update<AppMessage> {
        let content = alias_editor_content_area(self.conversation_pane.area);
        let input_area = Rect::new(content.x, content.y, content.width, 1);
        let editor = self.alias_editor.as_mut().expect("alias editor checked");
        let input =
            TextInputBox::new(bmux_tui_components::text_input::TextInputPolicy::chat_composer())
                .policy(TextInputBoxPolicy::field().focused(true))
                .placeholder("Local display name");
        match input.handle_event(input_area, &mut editor.input, event) {
            TextInputBoxOutcome::Submitted => {
                let value = editor.input.buffer().text().to_string();
                let resource_name = editor.resource_name.clone();
                match self
                    .aliases
                    .as_ref()
                    .map(|aliases| aliases.set(&resource_name, &value))
                {
                    Some(Ok(())) => {
                        apply_sender_aliases(self);
                        self.rebuild_projections();
                        self.alias_editor = None;
                    }
                    Some(Err(error)) => editor.error = Some(error.to_string()),
                    None => editor.error = Some("Local aliases are unavailable".to_string()),
                }
                Update::reset()
            }
            TextInputBoxOutcome::Edited | TextInputBoxOutcome::Redraw => Update::redraw(),
            TextInputBoxOutcome::Ignored
            | TextInputBoxOutcome::EdgeUp
            | TextInputBoxOutcome::EdgeDown => Update::none(),
        }
    }

    fn open_alias_editor(&mut self) -> Update<AppMessage> {
        self.help_visible = false;
        if !matches!(self.product.phase, Phase::Ready | Phase::Empty) {
            return Update::redraw();
        }
        let mut by_id = std::collections::BTreeMap::new();
        for sender in self
            .product
            .messages
            .iter()
            .filter_map(|message| message.sender.as_ref())
            .filter(|sender| !sender.resource_name.is_empty())
        {
            by_id
                .entry(sender.resource_name.clone())
                .or_insert_with(|| {
                    let label = sender_label(sender);
                    AliasSender {
                        resource_name: sender.resource_name.clone(),
                        label,
                    }
                });
        }
        let senders = by_id.into_values().collect::<Vec<_>>();
        if senders.is_empty() {
            return Update::redraw();
        }
        let items = senders
            .iter()
            .map(|sender| {
                SelectableListItem::multiline(
                    sender.resource_name.clone(),
                    vec![
                        Line::from_spans([Span::styled(
                            sender.label.clone(),
                            Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
                        )]),
                        Line::from_spans([Span::styled(
                            format!("  {}", sender.resource_name),
                            Style::new().fg(MUTED),
                        )]),
                    ],
                )
            })
            .collect();
        let mut state = SelectableListState::new(None);
        state.set_focused(Some(0));
        self.alias_picker = Some(AliasPicker {
            senders,
            items,
            state,
        });
        Update::reset()
    }

    fn handle_thread_activity_event(&mut self, event: &Event) -> Option<Update<AppMessage>> {
        let route = self.interactions.route(event.clone());
        let target = route.target.as_ref()?.as_str();
        let index = self
            .thread_activity_links
            .iter()
            .position(|link| link.id == target)?;
        let activated = matches!(
            event,
            Event::Mouse(bmux_tui::event::MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                ..
            })
        ) || event
            .key()
            .and_then(|stroke| self.bindings.action_for(stroke))
            == Some(Action::Activate);
        self.focused_thread_activity = Some(index);
        if activated {
            let area = conversation_content_area(self.conversation_pane.area);
            let target = wrapped_row_for_source_line(
                self.conversation_lines.as_slice(),
                area.width,
                self.thread_activity_links[index].target_line,
            );
            self.conversation_view.set_vertical_scroll(target);
            self.follow_conversation_bottom = false;
        }
        Some(Update::redraw())
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

    #[allow(clippy::too_many_lines)]
    fn update_terminal(&mut self, event: Event) -> Update<AppMessage> {
        if self.alias_picker.is_some() {
            if let Event::Key(stroke) = event
                && self.bindings.action_for(stroke) == Some(Action::Cancel)
            {
                self.alias_picker = None;
                return Update::reset();
            }
            return self.handle_alias_picker(&event);
        }
        if self.alias_editor.is_some() {
            if let Event::Key(stroke) = event
                && self.bindings.action_for(stroke) == Some(Action::Cancel)
            {
                self.alias_editor = None;
                return Update::reset();
            }
            return self.handle_alias_editor(&event);
        }
        if self.help_visible {
            if let Event::Key(stroke) = event
                && matches!(
                    self.bindings.action_for(stroke),
                    Some(Action::Help | Action::Cancel)
                )
            {
                self.help_visible = false;
                return Update::reset();
            }
            return Update::none();
        }
        if let Some(update) = self.handle_thread_activity_event(&event) {
            return update;
        }
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
            Action::Cancel => Update::none(),
            Action::Quit => Update {
                lifecycle: Lifecycle::Exit,
                ..Update::none()
            },
            Action::SetSenderAlias => self.open_alias_editor(),
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
            RuntimeEvent::Message(AppMessage::MessagesWithIdentity {
                message,
                current_user,
            }) => {
                self.product.update(message);
                apply_sender_aliases(self);
                infer_direct_message_name(self, current_user.as_ref());
                self.rebuild_projections();
                self.follow_conversation_bottom = true;
                sync_space_selection(self);
                Ok(Update::reset())
            }
            RuntimeEvent::Message(AppMessage::Product(message)) => {
                let messages_loaded = matches!(message, ProductMessage::MessagesLoaded { .. });
                self.product.update(message);
                apply_sender_aliases(self);
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

pub async fn run(
    bindings: KeybindingRegistry,
    access_token: Option<Secret>,
    aliases: Option<Arc<SenderAliases>>,
) -> Result<()> {
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
        app.aliases = aliases;
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

fn infer_direct_message_name(
    app: &mut App,
    current_user: Option<&(std::collections::BTreeSet<String>, Option<String>)>,
) {
    let Some(selected_id) = app.product.selected_space.as_deref() else {
        return;
    };
    let Some(space) = app
        .product
        .spaces
        .iter_mut()
        .find(|space| space.id.0 == selected_id)
    else {
        return;
    };
    if space.kind != crate::model::SpaceKind::DirectMessage || !space.display_name.trim().is_empty()
    {
        return;
    }
    let current_people = current_user
        .map(|(resource_names, _)| resource_names)
        .cloned()
        .unwrap_or_default();
    let candidate = app
        .product
        .messages
        .iter()
        .filter_map(|message| message.sender.as_ref())
        .filter(|sender| {
            chat_user_to_person_name(&sender.resource_name)
                .is_none_or(|person| !current_people.contains(&person))
        })
        .find_map(|sender| {
            sender.display_name.clone().or_else(|| {
                app.aliases
                    .as_ref()
                    .and_then(|aliases| aliases.get(&sender.resource_name))
            })
        });
    if let Some(name) = candidate {
        space.display_name = name;
    }
}

fn chat_user_to_person_name(resource_name: &str) -> Option<String> {
    resource_name
        .strip_prefix("users/")
        .filter(|id| !id.is_empty() && *id != "app")
        .map(|id| format!("people/{id}"))
}

fn apply_sender_aliases(app: &mut App) {
    let Some(aliases) = app.aliases.as_ref() else {
        return;
    };
    for message in &mut app.product.messages {
        if let Some(sender) = message.sender.as_mut() {
            sender.display_name =
                aliases.apply(&sender.resource_name, sender.display_name.as_deref());
        }
    }
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

fn project_conversation(product: &ProductState) -> TranscriptProjection {
    if product.messages.is_empty() {
        return TranscriptProjection {
            lines: vec![Line::from(match product.phase {
                Phase::LoadingMessages | Phase::Refreshing => "Loading messages…",
                Phase::Empty => "No messages",
                Phase::RecoverableError => "Unable to load messages. Refresh to retry.",
                Phase::Reauthentication => "Authorization expired. Sign in again.",
                _ => "Select a space to read messages",
            })],
            links: Vec::new(),
        };
    }
    crate::transcript_projection::project(
        &product.messages,
        sender_label,
        TranscriptColors {
            text: TEXT,
            muted: MUTED,
            accent: ACCENT_STRONG,
            warning: WARNING,
            border: BORDER,
        },
    )
}

fn sender_label(sender: &crate::model::Sender) -> String {
    if let Some(display_name) = sender
        .display_name
        .as_deref()
        .filter(|display_name| !display_name.trim().is_empty())
    {
        return display_name.to_string();
    }
    match sender.kind {
        crate::model::SenderKind::Bot => "Chat app".to_string(),
        crate::model::SenderKind::Anonymous => "Deleted user".to_string(),
        crate::model::SenderKind::Human | crate::model::SenderKind::Unknown => sender
            .resource_name
            .strip_prefix("users/")
            .filter(|id| !id.is_empty())
            .map_or_else(|| "Unknown sender".to_string(), |id| format!("User {id}")),
    }
}

fn wrapped_row_for_source_line(lines: &[Line], width: u16, source_line: usize) -> usize {
    lines
        .iter()
        .take(source_line)
        .map(|line| line.wrap_word(usize::from(width.max(1))).len().max(1))
        .sum()
}

fn thread_activity_area(
    link: &ThreadActivityLink,
    lines: &[Line],
    view: &TextView<'_>,
    state: &TextViewState,
    area: Rect,
) -> Option<Rect> {
    let layout = view.layout(area, state);
    let source_row = wrapped_row_for_source_line(lines, area.width, link.source_line);
    let relative = source_row.checked_sub(layout.vertical_scroll)?;
    let row = u16::try_from(relative).ok()?;
    (row < area.height).then_some(Rect::new(area.x, area.y.saturating_add(row), area.width, 1))
}

fn render_thread_activity_hits(app: &App, view: &TextView<'_>, area: Rect, frame: &mut Frame<'_>) {
    for link in app.thread_activity_links.iter() {
        let Some(hit_area) = thread_activity_area(
            link,
            app.conversation_lines.as_slice(),
            view,
            &app.conversation_view,
            area,
        ) else {
            continue;
        };
        frame.push_hit(
            HitRegion::new(HitId::new(link.id.clone()), hit_area)
                .role(HitRole::Action)
                .hoverable(true)
                .focusable(true),
        );
    }
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
    let list_area = space_list_area(spaces_area);
    frame
        .buffer_mut()
        .fill(list_area, " ", Style::new().bg(SURFACE));
    spaces_list(spaces.as_slice()).render_with_fallback_style(
        list_area,
        &app.spaces,
        frame,
        Style::new().fg(TEXT).bg(SURFACE),
    );

    let conversation = Arc::clone(&app.conversation_lines);
    let conversation_area = conversation_content_area(conversation_area);
    frame
        .buffer_mut()
        .fill(conversation_area, " ", Style::new().bg(SURFACE));
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
    render_thread_activity_hits(app, &conversation_view, conversation_area, frame);

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
    if app.alias_picker.is_some() {
        render_alias_picker(app, frame);
    }
    if app.alias_editor.is_some() {
        render_alias_editor(app, frame);
    }
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
    enforce_opaque_viewport(frame, area);
}

fn enforce_opaque_viewport(frame: &mut Frame<'_>, area: Rect) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let point = Point::new(x, y);
            let Some(cell) = frame.buffer_mut().get_mut(point) else {
                continue;
            };
            if cell.style.bg.is_none() || cell.style.bg == Some(Color::Default) {
                cell.style.bg = Some(CANVAS);
            }
            if cell.style.fg == Some(Color::Default) {
                cell.style.fg = Some(TEXT);
            }
        }
    }
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
        background: Style::new().bg(SURFACE),
        scrollbar: bmux_tui_components::scrollbar::ScrollbarStyles {
            begin: Style::new().fg(BORDER).bg(SURFACE),
            track: Style::new().fg(BORDER).bg(SURFACE),
            thumb: Style::new().fg(ACCENT).bg(SURFACE),
            end: Style::new().fg(BORDER).bg(SURFACE),
        },
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

fn alias_picker_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(6).clamp(24, 64);
    let height = area.height.saturating_sub(4).clamp(6, 18);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

fn alias_picker_content_area(area: Rect) -> Rect {
    let area = alias_picker_area(area);
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

fn render_alias_picker(app: &mut App, frame: &mut Frame<'_>) {
    let area = alias_picker_area(app.conversation_pane.area);
    let Some(picker) = app.alias_picker.as_mut() else {
        return;
    };
    let panel = Pane::new()
        .title("  CHOOSE SENDER  ·  ENTER SELECTS  ·  ESC CANCELS")
        .styles(PaneStyles {
            background: Some(Style::new().bg(SURFACE_RAISED)),
            border: Style::new().fg(ACCENT).bg(SURFACE_RAISED),
            focused_border: Style::new().fg(ACCENT_STRONG).bg(SURFACE_RAISED),
        });
    let state = PaneState::new(area);
    panel.render(&state, frame);
    spaces_list(&picker.items).render_with_fallback_style(
        panel.inner_area(&state),
        &picker.state,
        frame,
        Style::new().bg(SURFACE_RAISED).fg(TEXT),
    );
}

fn alias_editor_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).clamp(24, 56);
    let height = area.height.saturating_sub(4).clamp(7, 9);
    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y
            .saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

fn alias_editor_content_area(area: Rect) -> Rect {
    let area = alias_editor_area(area);
    Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(2),
        area.width.saturating_sub(4),
        area.height.saturating_sub(4),
    )
}

fn render_alias_editor(app: &mut App, frame: &mut Frame<'_>) {
    let area = alias_editor_area(app.conversation_pane.area);
    let Some(editor) = app.alias_editor.as_mut() else {
        return;
    };
    let panel = Pane::new()
        .title("  SET LOCAL SENDER NAME")
        .styles(PaneStyles {
            background: Some(Style::new().bg(SURFACE_RAISED)),
            border: Style::new().fg(ACCENT).bg(SURFACE_RAISED),
            focused_border: Style::new().fg(ACCENT_STRONG).bg(SURFACE_RAISED),
        });
    let state = PaneState::new(area);
    panel.render(&state, frame);
    let content = alias_editor_content_area(app.conversation_pane.area);
    frame
        .buffer_mut()
        .fill(content, " ", Style::new().bg(SURFACE_RAISED));
    let mut input =
        TextInputBox::new(bmux_tui_components::text_input::TextInputPolicy::chat_composer())
            .placeholder("Type a local display name…")
            .policy(TextInputBoxPolicy::bare().focused(true).rows(1, Some(1)));
    if let Some(error) = editor.error.as_deref() {
        input = input.error(error);
    }
    frame.buffer_mut().write_line(
        Rect::new(
            content.x,
            content.y.saturating_add(content.height.saturating_sub(1)),
            content.width,
            1,
        ),
        &Line::from_spans([Span::styled(
            "Enter saves  ·  Esc cancels",
            Style::new().fg(MUTED),
        )]),
    );
    let input_area = Rect::new(content.x, content.y, content.width, 1);
    input.render_with_id("sender-alias-input", input_area, &mut editor.input, frame);
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
fn render_to_buffer_and_hits(app: &mut App, area: Rect) -> (Buffer, HitMap) {
    let mut buffer = Buffer::empty(area);
    let mut frame = Frame::new(&mut buffer);
    render(app, &mut frame);
    let hits = frame.hits().clone();
    drop(frame);
    (buffer, hits)
}

#[cfg(test)]
fn render_to_buffer(app: &mut App, area: Rect) -> Buffer {
    render_to_buffer_and_hits(app, area).0
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

#[cfg(test)]
mod tests {
    use super::*;

    use bmux_tui::event::{MouseButton, MouseEvent, MouseEventKind};
    use bmux_tui::geometry::Point;

    #[test]
    fn text_view_code_viewports_are_stable_across_scroll_offsets() {
        let lines = vec![
            Line::from("fn main() {"),
            Line::from("    let value = very_long_function_name(argument_one, argument_two);"),
            Line::from("    println!(\"👩🏽‍💻 {value}\");"),
            Line::from("}"),
        ];
        let view = TextView::new(&lines);
        let area = Rect::new(0, 0, 24, 3);
        let canonical = view.layout(area, &TextViewState::new()).lines;
        for offset in 0..canonical.len() {
            let mut state = TextViewState::new();
            state.set_vertical_scroll(offset);
            let layout = view.layout(area, &state);
            assert_eq!(layout.lines, canonical);
            assert_eq!(
                layout.vertical_scroll,
                offset.min(canonical.len().saturating_sub(usize::from(area.height)))
            );
        }
    }

    #[test]
    fn first_sender_in_alias_picker_can_be_selected_immediately() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.phase = Phase::Ready;
        app.product.messages = vec![crate::model::Message {
            id: crate::model::MessageId("messages/example".to_string()),
            thread_id: None,
            sender: Some(crate::model::Sender {
                resource_name: "users/first".to_string(),
                display_name: None,
                kind: crate::model::SenderKind::Human,
            }),
            text: "Synthetic message".to_string(),
            create_time: "10:42".to_string(),
            is_thread_reply: false,
            unsupported_content: false,
        }];
        let _ = app.open_alias_editor();
        assert!(app.alias_picker.is_some());
        assert!(app.alias_editor.is_none());

        let enter = "Enter".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(enter.stroke()));
        assert!(app.alias_picker.is_none());
        assert_eq!(
            app.alias_editor
                .as_ref()
                .map(|editor| editor.resource_name.as_str()),
            Some("users/first")
        );
    }

    #[test]
    fn alias_editor_accepts_and_renders_typed_text() {
        let mut app = App::new(KeybindingRegistry::default());
        app.conversation_pane.area = Rect::new(30, 2, 70, 22);
        app.alias_editor = Some(AliasEditor {
            resource_name: "users/example".to_string(),
            input: TextInputState::default(),
            error: None,
        });
        for character in "Example Name".chars() {
            let stroke = crate::keybind::KeyChord::for_character(character.to_ascii_lowercase());
            let _ = app.update_terminal(Event::Key(stroke.stroke()));
        }
        assert_eq!(
            app.alias_editor.as_ref().unwrap().input.buffer().text(),
            "example name"
        );
        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 100, 26));
        let rendered = (0..26)
            .filter_map(|row| buffer.row_symbols(row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("example name"),
            "rendered frame:\n{rendered}"
        );
    }

    #[test]
    fn every_rendered_cell_has_an_explicit_opaque_background() {
        let mut app = App::new(KeybindingRegistry::default());
        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 100, 30));
        for cell in buffer.cells() {
            assert!(
                cell.style
                    .bg
                    .is_some_and(|background| background != Color::Default),
                "cell {:?} used the transparent terminal background",
                cell.symbol
            );
        }
    }

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
                is_thread_reply: false,
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
    fn thread_activity_click_jumps_to_wrapped_root() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.messages = vec![
            crate::model::Message {
                id: crate::model::MessageId("root".to_string()),
                thread_id: Some(crate::model::ThreadId("thread".to_string())),
                sender: None,
                text: "A root message with enough words to wrap across multiple terminal rows when rendered in a narrow conversation pane".to_string(),
                create_time: "09:00".to_string(),
                is_thread_reply: false,
                unsupported_content: false,
            },
            crate::model::Message {
                id: crate::model::MessageId("ordinary".to_string()),
                thread_id: None,
                sender: None,
                text: "ordinary".to_string(),
                create_time: "09:05".to_string(),
                is_thread_reply: false,
                unsupported_content: false,
            },
            crate::model::Message {
                id: crate::model::MessageId("reply".to_string()),
                thread_id: Some(crate::model::ThreadId("thread".to_string())),
                sender: None,
                text: "reply".to_string(),
                create_time: "09:10".to_string(),
                is_thread_reply: true,
                unsupported_content: false,
            },
        ];
        app.product.phase = Phase::Ready;
        app.rebuild_projections();
        app.conversation_view.set_vertical_scroll(usize::MAX);
        let (_buffer, hits) = render_to_buffer_and_hits(&mut app, Rect::new(0, 0, 70, 16));
        app.interactions.commit_scene(hits, None);
        let link = app.thread_activity_links.first().unwrap();
        let view = TextView::new(app.conversation_lines.as_slice());
        let area = conversation_content_area(app.conversation_pane.area);
        let hit_area = thread_activity_area(
            link,
            app.conversation_lines.as_slice(),
            &view,
            &app.conversation_view,
            area,
        )
        .unwrap();
        let point = Point::new(hit_area.x.saturating_add(2), hit_area.y);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Up(MouseButton::Left),
            point,
        )));
        assert_eq!(app.conversation_view.vertical_scroll(), 0);
    }

    #[test]
    fn multi_message_thread_groups_without_exposing_thread_id() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.messages = (0..2)
            .map(|index| crate::model::Message {
                id: crate::model::MessageId(format!("messages/{index}")),
                thread_id: Some(crate::model::ThreadId("threads/private-id".to_string())),
                sender: None,
                text: format!("Synthetic reply {index}"),
                create_time: format!("10:4{index}"),
                is_thread_reply: index > 0,
                unsupported_content: false,
            })
            .collect();
        app.rebuild_projections();
        let rendered = app
            .conversation_lines
            .iter()
            .map(Line::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("1 REPLY"));
        assert!(!rendered.contains("private-id"));
    }

    #[test]
    fn conversation_groups_thread_replies() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.messages = vec![crate::model::Message {
            id: crate::model::MessageId("messages/example".to_string()),
            thread_id: Some(crate::model::ThreadId("threads/example-thread".to_string())),
            sender: Some(crate::model::Sender {
                resource_name: "users/example".to_string(),
                display_name: Some("Example User".to_string()),
                kind: crate::model::SenderKind::Human,
            }),
            text: "Synthetic reply".to_string(),
            create_time: "10:42".to_string(),
            is_thread_reply: false,
            unsupported_content: false,
        }];
        app.rebuild_projections();
        let rendered = app
            .conversation_lines
            .iter()
            .map(Line::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("example-thread"));
        assert!(!rendered.contains("MESSAGES IN THREAD"));
        assert!(rendered.contains("Example User"));
        assert!(!rendered.contains("  Example User"));
        assert!(rendered.contains("Synthetic reply"));
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
    fn startup_does_not_render_placeholder_conversations() {
        let mut app = App::new(KeybindingRegistry::default());
        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 80, 20));
        let rendered = (0..20)
            .filter_map(|row| buffer.row_symbols(row))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("CONVERSATIONS"));
        assert!(!rendered.contains("Example Space"));
        assert!(!rendered.contains("Project Discussion"));
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
        assert!(!rendered.contains("Example Space"));
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
    fn selecting_real_space_opens_conversation_for_mouse_and_keyboard() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.spaces = vec![crate::model::Space {
            id: crate::model::SpaceId("spaces/example".to_string()),
            display_name: "Example Space".to_string(),
            kind: crate::model::SpaceKind::Space,
        }];
        app.product.phase = Phase::Ready;
        app.rebuild_projections();
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
}
