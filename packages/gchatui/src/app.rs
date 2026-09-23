use std::cell::{Cell, RefCell};
mod rich;
use std::io::{Stdout, stdout};
use std::sync::Arc;

use anyhow::Result;
#[cfg(test)]
use bmux_tui::buffer::Buffer;
use bmux_tui::component::{Component, Constraints, LayoutCx, LayoutId, LayoutNode, LogicalSize};
use bmux_tui::crossterm::{CrosstermTerminalGuard, terminal_size};
use bmux_tui::event::{Event, MouseButton, MouseEventKind};
#[cfg(test)]
use bmux_tui::frame::Frame;
use bmux_tui::geometry::{Insets, Rect, Size};
use bmux_tui::hit::{HitId, HitMap, HitRegion, HitRole};
use bmux_tui::interaction::InteractionRouter;
use bmux_tui::paint::{LocalRect, PaintCx};
use bmux_tui::prelude::{Color, Line, Modifier, Span, Style};
use bmux_tui::terminal::Terminal;
use bmux_tui_components::button::{
    Button, ButtonComponent, ButtonOutcome, ButtonState, ButtonStyles,
};
use bmux_tui_components::key_hint_bar::{
    KeyHint, KeyHintBarComponent, KeyHintBarPolicy, KeyHintBarStyles,
};
use bmux_tui_components::pane::{
    Pane, PaneComponent, PaneMousePolicy, PaneOutcome, PanePolicy, PaneState, PaneStyles,
};
use bmux_tui_components::scrollbar_layout::ScrollbarAxisLayoutMode;
use bmux_tui_components::selectable_list::{
    SelectableList, SelectableListHighlightPolicy, SelectableListItem, SelectableListOutcome,
    SelectableListPolicy, SelectableListState, SelectableListStyles,
};
use bmux_tui_components::status_bar::{
    StatusBarComponent, StatusBarPolicy, StatusBarStyles, StatusSegment, StatusSeverity,
};
use bmux_tui_components::text_input::{TextInputControl, TextInputOutcome, TextInputState};
use bmux_tui_components::text_input_box::{TextInputBoxComponent, TextInputBoxPolicy};
use bmux_tui_components::virtual_list::{VirtualList, VirtualListState};
use bmux_tui_runtime::{
    Command, CommandKey, ImageTerminalPresenter, Lifecycle, Program, Runtime, RuntimeConfig,
    RuntimeEvent, TerminalInput, Update,
};

use crate::chat::ChatClient;
use crate::credential::Secret;
use crate::keybind::{Action, KeybindingRegistry};
use crate::people::PeopleClient;
use crate::product::{Effect, Phase, ProductMessage, ProductState};
use crate::sender_alias::SenderAliases;
use crate::transcript_projection::{
    ThreadActivityLink, TranscriptColors, TranscriptItem, TranscriptProjection,
};

const CANVAS: Color = Color::Rgb(10, 14, 24);
const SURFACE: Color = Color::Rgb(16, 23, 38);
const MESSAGE_BG: Color = Color::Rgb(24, 35, 54);
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
    ImagesLoaded {
        generation: u64,
        space: Option<String>,
        images: crate::rich_content::Images,
    },
    Product(ProductMessage),
    MessagesWithIdentity {
        message: ProductMessage,
        current_user: Option<(std::collections::BTreeSet<String>, Option<String>)>,
    },
    NamesResolved {
        current_user: Option<(std::collections::BTreeSet<String>, Option<String>)>,
        request_id: u64,
        space_name: String,
        messages: Vec<crate::model::Message>,
    },
    Poll,
    Sweep,
    SweepFinished {
        result: Result<Vec<crate::model::Space>, crate::chat::ChatError>,
        activity: Vec<(String, Option<(crate::model::MessageId, String)>)>,
    },
    InputError(std::io::Error),
}

pub struct App {
    image_auth: Option<Arc<crate::auth::AuthManager>>,
    image_generation: u64,
    rich_items: Vec<(String, crate::model::RichContent)>,
    images: crate::rich_content::Images,
    image_protocol: Option<bmux_image::ImageProtocol>,
    rich_focus: Option<usize>,
    viewer: Option<rich::Viewer>,
    viewer_area: Rect,
    bindings: KeybindingRegistry,
    clock_format: crate::date_display::ClockFormat,
    timestamp_format: Option<crate::date_display::TimestampFormat>,
    interactions: InteractionRouter,
    spaces: SelectableListState,
    space_items: Arc<Vec<SelectableListItem>>,
    conversation_lines: Arc<Vec<Line>>,
    conversation_items: Arc<Vec<TranscriptItem>>,
    thread_activity_links: Arc<Vec<ThreadActivityLink>>,
    focused_thread_activity: Option<usize>,
    product: ProductState,
    chat: Arc<ChatClient>,
    people: Arc<PeopleClient>,
    aliases: Option<Arc<SenderAliases>>,
    access_token: Option<Arc<Secret>>,
    space_pane: PaneState,
    conversation_pane: PaneState,
    conversation_view: VirtualListState<String>,
    help_button: ButtonState,
    auth_button: ButtonState,
    auth_menu: bool,
    auth_selection: usize,
    auth_options: [ButtonState; 3],
    auth_result: Arc<std::sync::Mutex<Option<usize>>>,
    alias_picker: Option<AliasPicker>,
    alias_editor: Option<AliasEditor>,
    focused_pane: FocusedPane,
    follow_conversation_bottom: bool,
    help_visible: bool,
    poll_interval: u64,
    polling: Option<u64>,
    poll_page: Option<crate::model::PageToken>,
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
            image_auth: None,
            image_generation: 0,
            rich_items: Vec::new(),
            images: crate::rich_content::Images::new(),
            image_protocol: None,
            rich_focus: None,
            viewer: None,
            viewer_area: Rect::new(0, 0, 0, 0),
            bindings,
            clock_format: crate::date_display::ClockFormat::default(),
            timestamp_format: None,
            interactions: InteractionRouter::new(),
            spaces: SelectableListState::new(Some(0)),
            space_items: Arc::new(Vec::new()),
            conversation_lines: Arc::new(vec![Line::from("Select a space to read messages")]),
            conversation_items: Arc::new(vec![TranscriptItem {
                key: "transcript-line-0".to_string(),
                line: Line::from("Select a space to read messages"),
            }]),
            thread_activity_links: Arc::new(Vec::new()),
            focused_thread_activity: None,
            product: ProductState::default(),
            chat: Arc::new(ChatClient::new()),
            people: Arc::new(PeopleClient::new()),
            aliases: None,
            access_token: None,
            space_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_pane: PaneState::new(Rect::new(0, 0, 0, 0)),
            conversation_view: VirtualListState::new(0),
            help_button: ButtonState::new(),
            auth_button: ButtonState::new(),
            auth_menu: false,
            auth_selection: 0,
            auth_options: [ButtonState::new(); 3],
            auth_result: Arc::new(std::sync::Mutex::new(None)),
            alias_picker: None,
            alias_editor: None,
            focused_pane: FocusedPane::Spaces,
            follow_conversation_bottom: false,
            help_visible: false,
            poll_interval: 5,
            polling: None,
            poll_page: None,
        }
    }

    fn rebuild_projections(&mut self) {
        self.space_items = Arc::new(project_spaces(&self.product));
        let projection = project_conversation(
            &self.product,
            self.clock_format,
            self.timestamp_format.as_ref(),
        );
        if self.rich_items != projection.rich {
            self.rich_focus = None;
            self.viewer = None;
        }
        self.rich_items = projection.rich;
        self.conversation_lines = Arc::new(projection.lines);
        self.conversation_items = Arc::new(projection.items);
        self.thread_activity_links = Arc::new(projection.links);
        self.focused_thread_activity = None;
    }

    const fn focus_spaces_pane(&mut self) {
        self.focused_pane = FocusedPane::Spaces;
        self.space_pane.interaction.focused = true;
        self.conversation_pane.interaction.focused = false;
    }

    fn apply_sweep_activity(
        &mut self,
        activity: Vec<(String, Option<(crate::model::MessageId, String)>)>,
    ) {
        for (space, latest) in activity {
            self.product
                .observe_activity(&space, latest.as_ref().map(|(id, _)| id));
            if let Some((_, timestamp)) = latest {
                self.product.observe_recency(&space, &timestamp);
            }
        }
        self.product.sort_spaces_by_recency();
    }

    fn start_sweep(&self) -> Update<AppMessage> {
        if self.auth_menu
            || self.access_token.is_none()
            || self.product.phase == Phase::Reauthentication
        {
            return Update::none().with_command(sweep_timer(60));
        }
        let chat = Arc::clone(&self.chat);
        let token = self.access_token.clone().expect("token checked");
        Update::none().with_command(Command::replace(
            CommandKey::new("space-sweep"),
            async move {
                let result = chat.list_all_spaces(token.as_ref()).await;
                let mut activity = Vec::new();
                if let Ok(spaces) = &result {
                    for space in spaces {
                        // Pace background requests; no burst proportional to workspace size.
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        match chat.list_messages(token.as_ref(), &space.id, 1, None).await {
                            Ok(page) => activity.push((
                                space.id.0.clone(),
                                page.items.last().map(|message| {
                                    (message.id.clone(), message.create_time.clone())
                                }),
                            )),
                            Err(
                                crate::chat::ChatError::Unauthorized
                                | crate::chat::ChatError::RateLimited,
                            ) => break,
                            Err(_) => {}
                        }
                    }
                }
                Some(AppMessage::SweepFinished { result, activity })
            },
        ))
    }

    fn apply_product_message(&mut self, message: ProductMessage) -> Update<AppMessage> {
        let messages_loaded = matches!(message, ProductMessage::MessagesLoaded { .. });
        if !self.product.accepts(&message) {
            return Update::none();
        }
        let was_poll = matches!(&message, ProductMessage::MessagesLoaded { request_id, .. } if self.polling == Some(*request_id));
        let previous = self.product.messages.clone();
        self.product.update(message);
        if was_poll {
            self.poll_interval = if self.product.messages != previous || self.poll_interval < 5 {
                5
            } else {
                (self.poll_interval * 2).min(60)
            };
            self.product.next_message_page = self.poll_page.take();
            self.polling = None;
        }
        apply_sender_aliases(self);
        self.rebuild_projections();
        if !was_poll && messages_loaded && matches!(self.product.phase, Phase::Ready | Phase::Empty)
        {
            self.follow_conversation_bottom = true;
        }
        sync_space_selection(self);
        if messages_loaded {
            Update::reset().with_command(self.load_images())
        } else {
            Update::reset()
        }
    }

    fn enrich_message(&self, message: &ProductMessage) -> Option<Command<AppMessage>> {
        let ProductMessage::MessagesLoaded {
            request_id,
            space_name,
            result: Ok((messages, _)),
        } = message
        else {
            return None;
        };
        let (request_id, space_name, mut messages) =
            (*request_id, space_name.clone(), messages.clone());
        let people = Arc::clone(&self.people);
        let token = self.access_token.clone()?;
        Some(Command::replace(
            CommandKey::new("sender-names"),
            async move {
                let _ = people
                    .resolve_message_senders(token.as_ref(), &mut messages)
                    .await;
                let current_user = people.current_user(token.as_ref()).await.ok();
                Some(AppMessage::NamesResolved {
                    current_user,
                    request_id,
                    space_name,
                    messages,
                })
            },
        ))
    }

    fn command_for_effect(&mut self, effect: Effect) -> Option<Command<AppMessage>> {
        self.image_generation = self.image_generation.wrapping_add(1);
        self.viewer = None;
        let token = self.access_token.clone()?;
        let chat = Arc::clone(&self.chat);
        let people = Arc::clone(&self.people);
        let aliases = self.aliases.clone();
        match effect {
            Effect::LoadSpaces { request_id } => {
                Some(Command::replace(CommandKey::new("spaces"), async move {
                    let result = match chat.list_all_spaces(token.as_ref()).await {
                        Ok(mut spaces) => {
                            chat.infer_direct_message_titles(
                                token.as_ref(),
                                &mut spaces,
                                &people,
                                aliases.as_deref(),
                            )
                            .await;
                            Ok((spaces, None))
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
                Some(AppMessage::MessagesWithIdentity {
                    message: ProductMessage::MessagesLoaded {
                        request_id,
                        space_name,
                        result: Ok((page.items, page.next_page_token)),
                    },
                    current_user: None,
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
        let _ = input_area;
        let input_policy = bmux_tui_components::text_input::TextInputPolicy::chat_composer();
        let input = TextInputControl::new(&input_policy);
        match input.handle_event(&mut editor.input, event) {
            TextInputOutcome::Submitted => {
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
            TextInputOutcome::Edited | TextInputOutcome::Redraw => Update::redraw(),
            TextInputOutcome::Ignored | TextInputOutcome::EdgeUp | TextInputOutcome::EdgeDown => {
                Update::none()
            }
        }
    }

    fn handle_auth_menu(&mut self, event: &Event) -> Update<AppMessage> {
        if let Event::Key(stroke) = event {
            match self.bindings.action_for(*stroke) {
                Some(Action::Quit) => return self.apply_action(Action::Quit),
                Some(Action::Cancel) => {
                    self.auth_menu = false;
                    return Update::reset();
                }
                Some(Action::MoveDown | Action::FocusNext) => {
                    self.auth_selection = (self.auth_selection + 1) % 3;
                }
                Some(Action::MoveUp | Action::FocusPrevious) => {
                    self.auth_selection = (self.auth_selection + 2) % 3;
                }
                Some(Action::Activate) => return self.choose_auth(self.auth_selection),
                _ => {}
            }
            return Update::reset();
        }
        for (index, label) in STORAGE_OPTIONS.iter().enumerate() {
            if Button::new(label).handle_event(
                auth_option_area(self.conversation_pane.area, index),
                &mut self.auth_options[index],
                event,
            ) == ButtonOutcome::Pressed
            {
                return self.choose_auth(index);
            }
        }
        Update::reset()
    }

    fn choose_auth(&self, selection: usize) -> Update<AppMessage> {
        *self.auth_result.lock().expect("auth result lock") = Some(selection);
        Update {
            lifecycle: Lifecycle::Exit,
            ..Update::none()
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
        if let Some(update) = self.rich_target(target, event) {
            return Some(update);
        }
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
            let target_key = self.thread_activity_links[index].target_key.clone();
            self.conversation_view
                .scroll_to_key(&target_key, u64::from(area.height));
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
                i32::try_from(self.conversation_view.scroll.vertical_offset()).unwrap_or(i32::MAX);
            let next = u64::try_from(current.saturating_add(delta).max(0)).unwrap_or(u64::MAX);
            if next == self.conversation_view.scroll.vertical_offset() {
                return Some(Update::none());
            }
            self.conversation_view.scroll.set_vertical_offset(next);
            self.follow_conversation_bottom = false;
            return Some(Update::redraw());
        }
        if self.space_pane.area.contains(mouse.position) {
            let current = i32::try_from(self.spaces.vertical_scroll()).unwrap_or(i32::MAX);
            let next = u64::try_from(current.saturating_add(delta).max(0)).unwrap_or(u64::MAX);
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
        if let Some(update) = self.handle_rich(&event) {
            return update;
        }
        if self.auth_menu {
            return self.handle_auth_menu(&event);
        }
        if matches!(event, Event::Mouse(_))
            && Button::new("Sign in / storage").handle_event(
                auth_button_area(footer_area(self)),
                &mut self.auth_button,
                &event,
            ) == ButtonOutcome::Pressed
        {
            self.auth_menu = true;
            return Update::reset();
        }
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
                let amount = u64::from(area.height.max(1));
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
                let area = conversation_content_area(self.conversation_pane.area);
                let amount = i64::from(area.height.max(1));
                match action {
                    Action::PageDown => self.conversation_view.scroll.set_vertical_offset(
                        self.conversation_view
                            .scroll
                            .vertical_offset()
                            .saturating_add_signed(amount),
                    ),
                    Action::PageUp => self.conversation_view.scroll.set_vertical_offset(
                        self.conversation_view
                            .scroll
                            .vertical_offset()
                            .saturating_add_signed(-amount),
                    ),
                    Action::GoTop => self.conversation_view.scroll.set_vertical_offset(0),
                    Action::GoBottom => {
                        self.conversation_view.scroll.set_vertical_offset(u64::MAX);
                        self.conversation_view.scroll.set_follow_bottom(true);
                    }
                    _ => {}
                }
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
            Action::Authenticate => {
                self.auth_menu = true;
                Update::reset()
            }
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
                Update::reset()
            }
            Action::MoveDown if self.focused_pane == FocusedPane::Conversation => {
                self.follow_conversation_bottom = false;
                self.conversation_view.scroll.set_vertical_offset(
                    self.conversation_view
                        .scroll
                        .vertical_offset()
                        .saturating_add(1),
                );
                Update::redraw()
            }
            Action::MoveUp if self.focused_pane == FocusedPane::Conversation => {
                self.follow_conversation_bottom = false;
                self.conversation_view.scroll.set_vertical_offset(
                    self.conversation_view
                        .scroll
                        .vertical_offset()
                        .saturating_sub(1),
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
            RuntimeEvent::Message(AppMessage::Start) => Ok(startup_update(self)
                .with_command(poll_timer(5))
                .with_command(sweep_timer(1))),
            RuntimeEvent::Message(AppMessage::Sweep) => Ok(self.start_sweep()),
            RuntimeEvent::Message(AppMessage::SweepFinished { result, activity }) => {
                let delay = if result.is_ok() { 60 } else { 120 };
                if let Ok(spaces) = result {
                    self.product.reconcile_spaces(spaces);
                    self.apply_sweep_activity(activity);
                    self.rebuild_projections();
                    sync_space_selection(self);
                }
                Ok(Update::reset().with_command(sweep_timer(delay)))
            }
            RuntimeEvent::Message(AppMessage::Poll) => {
                if !self.auth_menu
                    && self.access_token.is_some()
                    && let Some(effect) = self.product.poll()
                {
                    if let Effect::LoadMessages { request_id, .. } = &effect {
                        self.polling = Some(*request_id);
                    }
                    self.poll_page = self.product.next_message_page.clone();
                    if let Some(command) = self.command_for_effect(effect) {
                        return Ok(Update::none()
                            .with_command(command)
                            .with_command(poll_timer(self.poll_interval)));
                    }
                }
                Ok(Update::none().with_command(poll_timer(self.poll_interval)))
            }
            RuntimeEvent::Message(AppMessage::MessagesWithIdentity {
                message,
                current_user,
            }) => {
                if !self.product.accepts(&message) {
                    return Ok(Update::none());
                }
                let was_poll = matches!(&message, ProductMessage::MessagesLoaded { request_id, .. } if self.polling == Some(*request_id));
                let enrichment = self.enrich_message(&message);
                let previous = self.product.messages.clone();
                self.product.update(message);
                if was_poll {
                    self.poll_interval =
                        if self.product.messages != previous || self.poll_interval < 5 {
                            5
                        } else {
                            (self.poll_interval * 2).min(60)
                        };
                    self.product.next_message_page = self.poll_page.take();
                    self.polling = None;
                }
                apply_sender_aliases(self);
                infer_direct_message_name(self, current_user.as_ref());
                self.rebuild_projections();
                if !was_poll {
                    self.follow_conversation_bottom = true;
                }
                sync_space_selection(self);
                Ok(enrichment
                    .map_or_else(Update::reset, |command| {
                        Update::reset().with_command(command)
                    })
                    .with_command(self.load_images()))
            }
            RuntimeEvent::Message(AppMessage::NamesResolved {
                current_user,
                request_id,
                space_name,
                messages,
            }) => {
                if !self.product.apply_names(request_id, &space_name, &messages) {
                    return Ok(Update::none());
                }
                infer_direct_message_name(self, current_user.as_ref());
                apply_sender_aliases(self);
                self.rebuild_projections();
                Ok(Update::redraw())
            }
            RuntimeEvent::Message(AppMessage::Product(message)) => {
                Ok(self.apply_product_message(message))
            }
            RuntimeEvent::Message(AppMessage::ImagesLoaded {
                generation,
                space,
                images,
            }) => {
                if generation == self.image_generation && space == self.product.selected_space {
                    self.images = images;
                }
                Ok(Update::redraw())
            }
            RuntimeEvent::Message(AppMessage::InputError(error)) => Err(error),
            RuntimeEvent::Timer(_) => Ok(Update::none()),
        }
    }
}

fn sweep_timer(seconds: u64) -> Command<AppMessage> {
    Command::replace(CommandKey::new("sweep-timer"), async move {
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
        Some(AppMessage::Sweep)
    })
}

fn poll_timer(seconds: u64) -> Command<AppMessage> {
    Command::replace(CommandKey::new("poll-timer"), async move {
        tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
        Some(AppMessage::Poll)
    })
}

pub async fn run(
    bindings: KeybindingRegistry,
    clock_format: crate::date_display::ClockFormat,
    timestamp_format: Option<crate::date_display::TimestampFormat>,
    access_token: Option<Arc<crate::auth::AuthManager>>,
    aliases: Option<Arc<SenderAliases>>,
    show_auth: bool,
) -> Result<Option<usize>> {
    let auth_result = Arc::new(std::sync::Mutex::new(None));
    let mut guard = CrosstermTerminalGuard::enter(stdout())?;
    let result = {
        let writer = guard.writer_mut().expect("guard should own stdout");
        let size = terminal_size()?;
        let terminal = Terminal::new(writer, Rect::new(0, 0, size.width, size.height));
        let capabilities = bmux_image::host_caps::detect_from_env();
        let image_protocol = capabilities.preferred_protocol();
        let presenter = ImageTerminalPresenter::with_commit(
            terminal,
            render,
            |app: &mut App, hits: &HitMap, _focus: &bmux_tui::focus::FocusTrap| {
                app.interactions.commit_scene(hits.clone(), None);
            },
            capabilities,
            bmux_image::ImageConfig::default(),
        );
        let mut app = App::new(bindings);
        app.image_protocol = image_protocol;
        app.clock_format = clock_format;
        app.timestamp_format = timestamp_format;
        app.aliases = aliases;
        app.auth_menu = show_auth;
        app.auth_result = Arc::clone(&auth_result);
        let startup = access_token.map(|auth| {
            app.image_auth = Some(Arc::clone(&auth));
            app.chat = Arc::new(ChatClient::with_auth(Arc::clone(&auth)));
            app.people = Arc::new(PeopleClient::with_auth(auth));
            app.access_token = Some(Arc::new(zeroize::Zeroizing::new(String::new())));
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
            Ok(mut output) => output
                .presenter
                .cleanup_images()
                .map_err(anyhow::Error::from),
            Err(bmux_tui_runtime::RuntimeError::Program { error, mut output }) => {
                let _ = output.presenter.cleanup_images();
                Err(anyhow::Error::from(error))
            }
            Err(bmux_tui_runtime::RuntimeError::Presenter { error, mut output }) => {
                let _ = output.presenter.cleanup_images();
                Err(anyhow::Error::from(error))
            }
        }
    };
    let _stdout: Stdout = guard.leave()?;
    result?;
    let selected = *auth_result.lock().expect("auth result lock");
    Ok(selected)
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
        .product
        .selected_space
        .as_ref()
        .and_then(|id| {
            app.product
                .spaces
                .iter()
                .position(|space| &space.id.0 == id)
        })
        .or_else(|| app.spaces.selected())
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
                            format!(
                                "{}{}",
                                if product.new_activity.contains(&space.id.0) {
                                    "[new] "
                                } else {
                                    ""
                                },
                                space.display_name
                            ),
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

fn project_conversation(
    product: &ProductState,
    clock: crate::date_display::ClockFormat,
    custom: Option<&crate::date_display::TimestampFormat>,
) -> TranscriptProjection {
    if product.messages.is_empty() {
        let line = Line::from(match product.phase {
            Phase::LoadingMessages | Phase::Refreshing => "Loading messages…",
            Phase::Empty => "No messages",
            Phase::RecoverableError => "Unable to load messages. Refresh to retry.",
            Phase::Reauthentication => "Authorization expired. Sign in again.",
            _ => "Select a space to read messages",
        });
        return TranscriptProjection {
            lines: vec![line.clone()],
            items: vec![TranscriptItem {
                key: "transcript-line-0".to_string(),
                line,
            }],
            links: Vec::new(),
            rich: Vec::new(),
        };
    }
    let mut display_messages = product.messages.clone();
    for message in &mut display_messages {
        message.create_time = custom.map_or_else(
            || clock.format_local(&message.create_time),
            |format| format.format_local(&message.create_time),
        );
    }
    crate::transcript_projection::project(
        &display_messages,
        sender_label,
        TranscriptColors {
            text: TEXT,
            muted: MUTED,
            accent: ACCENT_STRONG,
            warning: WARNING,
            border: BORDER,
            message_background: MESSAGE_BG,
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

fn transcript_list(items: &[TranscriptItem]) -> VirtualList<'_, String> {
    items
        .iter()
        .fold(VirtualList::new("conversation"), |list, item| {
            let background = if item
                .line
                .spans
                .iter()
                .any(|span| span.style.bg == Some(MESSAGE_BG))
            {
                MESSAGE_BG
            } else {
                SURFACE
            };
            list.item(
                item.key.clone(),
                0,
                bmux_tui::prelude::TextBlock::new(bmux_tui::prelude::Text::from_lines([item
                    .line
                    .clone()]))
                .style(Style::new().fg(TEXT).bg(background)),
            )
        })
}

fn thread_activity_area(
    link: &ThreadActivityLink,
    state: &VirtualListState<String>,
    area: Rect,
) -> Option<Rect> {
    let source = state.item_offset(&link.source_key)?;
    let relative = source.checked_sub(state.scroll.vertical_offset())?;
    let row = u16::try_from(relative).ok()?;
    (row < area.height).then_some(Rect::new(area.x, area.y.saturating_add(row), area.width, 1))
}

fn render_thread_activity_hits(app: &App, area: Rect, cx: &mut PaintCx<'_, '_>) {
    for link in app.thread_activity_links.iter() {
        let Some(hit_area) = thread_activity_area(link, &app.conversation_view, area) else {
            continue;
        };
        cx.push_hit(
            HitRegion::new(HitId::new(link.id.clone()), hit_area)
                .role(HitRole::Action)
                .hoverable(true)
                .focusable(true),
        );
    }
}

struct EmptyComponent;

impl Component for EmptyComponent {
    fn layout(&self, constraints: Constraints, cx: &mut LayoutCx) -> LayoutNode {
        cx.record_measurement();
        LayoutNode::leaf(
            LayoutId::new("empty"),
            constraints.constrain(LogicalSize::new(0, 0)),
        )
    }

    fn paint(&self, _layout: &LayoutNode, _cx: &mut PaintCx<'_, '_>) {}
}

fn raster_area(cx: &PaintCx<'_, '_>) -> Rect {
    cx.project_raster_rect(cx.area())
        .unwrap_or(Rect::new(0, 0, 0, 0))
}

fn paint_component(frame: &mut PaintCx<'_, '_>, area: Rect, component: &impl Component) {
    if area.is_empty() {
        return;
    }
    let layout = component.layout(
        Constraints::tight(Size::new(area.width, area.height)),
        &mut LayoutCx::new(),
    );
    frame.with_child(
        i32::from(area.x),
        i64::from(area.y),
        LocalRect::new(0, 0, area.width, area.height),
        |cx| component.paint(&layout, cx),
    );
}

#[allow(clippy::too_many_lines)]
fn render(app: &mut App, frame: &mut PaintCx<'_, '_>) {
    let area = raster_area(frame);
    frame.fill(
        LocalRect::terminal(area),
        " ",
        Style::new().bg(CANVAS).fg(TEXT),
    );
    if area.width < 20 || area.height < 6 {
        paint_component(
            frame,
            area,
            &bmux_tui::prelude::TextBlock::new(bmux_tui::prelude::Text::from_lines([Line::from(
                "Terminal is too small",
            )]))
            .wrap(bmux_tui::text::TextWrap::None)
            .style(Style::new().fg(TEXT).bg(CANVAS)),
        );
        return;
    }

    let header_height = 2;
    let footer_height = 2;
    let body_height = area.height.saturating_sub(header_height + footer_height);
    paint_component(
        frame,
        Rect::new(area.x, area.y, area.width, header_height),
        &HeaderComponent(app),
    );
    let body_y = area.y.saturating_add(header_height);
    let spaces_width = (area.width / 3).clamp(22, 38).min(area.width);
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
    let spaces_pane = pane.clone().title(Line::from_spans(vec![
        Span::styled(
            "  CONVERSATIONS",
            Style::new()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", app.product.spaces.len()),
            Style::new().fg(MUTED).bg(SURFACE),
        ),
        Span::styled(" ".repeat(100), Style::new().bg(SURFACE)),
    ]));
    paint_component(
        frame,
        spaces_area,
        &PaneComponent::new(
            "spaces-pane",
            spaces_pane,
            &Cell::new(app.space_pane),
            EmptyComponent,
        ),
    );
    let conversation_pane = pane.title(Line::from_spans(vec![
        Span::styled(
            "  MESSAGES",
            Style::new()
                .fg(ACCENT)
                .bg(SURFACE)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            selected_space_label(app),
            Style::new().fg(MUTED).bg(SURFACE),
        ),
        Span::styled(" ".repeat(100), Style::new().bg(SURFACE)),
    ]));
    paint_component(
        frame,
        conversation_area,
        &PaneComponent::new(
            "conversation-pane",
            conversation_pane,
            &Cell::new(app.conversation_pane),
            EmptyComponent,
        ),
    );

    let spaces = Arc::clone(&app.space_items);
    let list_area = space_list_area(spaces_area);
    frame.fill(
        LocalRect::terminal(list_area),
        " ",
        Style::new().bg(SURFACE),
    );
    spaces_list(spaces.as_slice()).paint(
        list_area,
        &app.spaces,
        Style::new().fg(TEXT).bg(SURFACE),
        frame,
    );

    let conversation_area = conversation_content_area(conversation_area);
    frame.fill(
        LocalRect::terminal(conversation_area),
        " ",
        Style::new().bg(SURFACE),
    );
    let conversation_items = Arc::clone(&app.conversation_items);
    let conversation_list = transcript_list(conversation_items.as_slice());
    app.conversation_view.capture_anchor();
    conversation_list.sync(
        u64::from(conversation_area.width),
        &mut app.conversation_view,
        &mut LayoutCx::new(),
    );
    app.conversation_view
        .restore_anchor(u64::from(conversation_area.height));
    if app.follow_conversation_bottom {
        app.conversation_view.scroll.set_vertical_offset(u64::MAX);
        app.conversation_view.scroll.set_follow_bottom(true);
        app.follow_conversation_bottom = false;
    }
    frame.with_child(
        i32::from(conversation_area.x),
        i64::from(conversation_area.y),
        LocalRect::new(0, 0, conversation_area.width, conversation_area.height),
        |cx| {
            conversation_list.paint(
                Rect::new(0, 0, conversation_area.width, conversation_area.height),
                &app.conversation_view,
                cx,
            );
            render_thread_activity_hits(
                app,
                Rect::new(0, 0, conversation_area.width, conversation_area.height),
                cx,
            );
        },
    );

    rich::paint_inline(app, frame, conversation_area);
    let footer_y = area
        .y
        .saturating_add(header_height)
        .saturating_add(body_height);
    let hint_labels = hints(app);
    let hints = hint_labels
        .iter()
        .map(|(key, label)| KeyHint::new(key, label))
        .collect::<Vec<_>>();
    paint_component(
        frame,
        Rect::new(area.x, footer_y, area.width, 1),
        &KeyHintBarComponent::new("key-hints", &hints)
            .policy(KeyHintBarPolicy::compact())
            .styles(key_hint_styles()),
    );
    paint_component(
        frame,
        help_button_area(Rect::new(area.x, footer_y, area.width, 1)),
        &ButtonComponent::new("help-button", "  ? HELP  ", &Cell::new(app.help_button))
            .styles(button_styles()),
    );
    render_auth_button(app, frame);
    if app.auth_menu {
        render_auth_menu(app, frame);
    }
    if app.alias_picker.is_some() {
        render_alias_picker(app, frame);
    }
    if app.alias_editor.is_some() {
        render_alias_editor(app, frame);
    }
    if app.help_visible {
        paint_component(frame, app.conversation_pane.area, &HelpComponent(app));
    }
    rich::paint_viewer(app, frame);
    let status_text = status_text(app);
    let severity = status_severity(app.product.phase);
    let status = [StatusSegment::new(status_text).severity(severity)];
    let right = [StatusSegment::new("READ ONLY").severity(StatusSeverity::Muted)];
    paint_component(
        frame,
        Rect::new(area.x, footer_y.saturating_add(1), area.width, 1),
        &StatusBarComponent::new("status-bar")
            .left(&status)
            .right(&right)
            .policy(StatusBarPolicy::compact().background(true))
            .styles(status_styles()),
    );
}

struct HeaderComponent<'a>(&'a App);

impl Component for HeaderComponent<'_> {
    fn layout(&self, constraints: Constraints, cx: &mut LayoutCx) -> LayoutNode {
        cx.record_measurement();
        let width = header_lines(self.0)
            .iter()
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .saturating_add(2);
        LayoutNode::leaf(
            LayoutId::new("header"),
            constraints.constrain(LogicalSize::new(
                u64::try_from(width).unwrap_or(u64::MAX),
                2,
            )),
        )
    }

    fn paint(&self, layout: &LayoutNode, cx: &mut PaintCx<'_, '_>) {
        render_header(
            self.0,
            cx,
            Rect::new(
                0,
                0,
                u16::try_from(layout.size.width).unwrap_or(u16::MAX),
                u16::try_from(layout.size.height).unwrap_or(u16::MAX),
            ),
        );
    }
}

fn header_lines(app: &App) -> [Line; 2] {
    [
        Line::from_spans(vec![
            Span::styled(
                "gchat",
                Style::new()
                    .fg(ACCENT_STRONG)
                    .bg(SURFACE_RAISED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "ui",
                Style::new()
                    .fg(TEXT)
                    .bg(SURFACE_RAISED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  /  terminal conversations",
                Style::new().fg(MUTED).bg(SURFACE_RAISED),
            ),
        ]),
        Line::from_spans(vec![
            Span::styled(
                "● ",
                Style::new()
                    .fg(status_color(app.product.phase))
                    .bg(SURFACE_RAISED),
            ),
            Span::styled(status_text(app), Style::new().fg(MUTED).bg(SURFACE_RAISED)),
        ]),
    ]
}

fn render_header(app: &App, cx: &mut PaintCx<'_, '_>, area: Rect) {
    cx.fill(
        LocalRect::terminal(area),
        " ",
        Style::new().bg(SURFACE_RAISED),
    );
    for (row, line) in (0..area.height).zip(header_lines(app)) {
        cx.write_line_with_fallback_style(
            LocalRect::new(
                i32::from(area.x) + 1,
                i64::from(area.y) + i64::from(row),
                area.width.saturating_sub(2),
                1,
            ),
            &line,
            Style::new().fg(TEXT).bg(SURFACE_RAISED),
        );
    }
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
            ..SelectableListPolicy::interactive().scrollbar(ScrollbarAxisLayoutMode::Gutter)
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

const STORAGE_OPTIONS: [&str; 3] = [
    "Sign in: session only (no saved token)",
    "Sign in: vault with passphrase",
    "Sign in: vault without passphrase",
];

fn auth_button_area(footer: Rect) -> Rect {
    Rect::new(footer.x, footer.y, footer.width.min(21), 1)
}

fn auth_option_area(area: Rect, index: usize) -> Rect {
    let area = alias_picker_area(area);
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(2 + u16::try_from(index).unwrap_or(0)),
        area.width.saturating_sub(2),
        1,
    )
}

fn render_auth_button(app: &App, frame: &mut PaintCx<'_, '_>) {
    paint_component(
        frame,
        auth_button_area(footer_area(app)),
        &ButtonComponent::new(
            "auth-button",
            "Sign in / storage",
            &Cell::new(app.auth_button),
        )
        .styles(button_styles()),
    );
}

fn render_auth_menu(app: &App, frame: &mut PaintCx<'_, '_>) {
    let area = alias_picker_area(app.conversation_pane.area);
    let title = format!(
        "Sign in · {} selects · {} cancels",
        app.bindings.labels_for(Action::Activate).join("/"),
        app.bindings.labels_for(Action::Cancel).join("/")
    );
    let pane = Pane::new().title(Line::from(title)).styles(PaneStyles {
        background: Some(Style::new().bg(SURFACE_RAISED)),
        border: Style::new().fg(ACCENT),
        focused_border: Style::new().fg(ACCENT),
    });
    paint_component(
        frame,
        area,
        &PaneComponent::new(
            "auth-menu",
            pane,
            &Cell::new(PaneState::new(area)),
            EmptyComponent,
        ),
    );
    for (index, label) in STORAGE_OPTIONS.iter().enumerate() {
        let text = format!(
            "{} {label}",
            if app.auth_selection == index {
                "→"
            } else {
                " "
            }
        );
        paint_component(
            frame,
            auth_option_area(app.conversation_pane.area, index),
            &ButtonComponent::new(
                format!("auth-option-{index}"),
                &text,
                &Cell::new(app.auth_options[index]),
            )
            .styles(button_styles()),
        );
    }
}

fn render_alias_picker(app: &mut App, frame: &mut PaintCx<'_, '_>) {
    let area = alias_picker_area(app.conversation_pane.area);
    let Some(picker) = app.alias_picker.as_mut() else {
        return;
    };
    let panel = Pane::new()
        .title(Line::from_spans([
            Span::styled(
                "  CHOOSE SENDER  ·  ENTER SELECTS  ·  ESC CANCELS",
                Style::new().fg(TEXT).bg(SURFACE_RAISED),
            ),
            Span::styled(" ".repeat(100), Style::new().bg(SURFACE_RAISED)),
        ]))
        .styles(PaneStyles {
            background: Some(Style::new().bg(SURFACE_RAISED)),
            border: Style::new().fg(ACCENT).bg(SURFACE_RAISED),
            focused_border: Style::new().fg(ACCENT_STRONG).bg(SURFACE_RAISED),
        });
    let pane_state = PaneState::new(area);
    let inner_area = panel.inner_area(&pane_state);
    let state = Cell::new(pane_state);
    paint_component(
        frame,
        area,
        &PaneComponent::new("dialog-pane", panel, &state, EmptyComponent),
    );
    spaces_list(&picker.items).paint(
        inner_area,
        &picker.state,
        Style::new().bg(SURFACE_RAISED).fg(TEXT),
        frame,
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

fn render_alias_editor(app: &mut App, frame: &mut PaintCx<'_, '_>) {
    let area = alias_editor_area(app.conversation_pane.area);
    let Some(editor) = app.alias_editor.as_mut() else {
        return;
    };
    let panel = Pane::new()
        .title(Line::from_spans([
            Span::styled(
                "  SET LOCAL SENDER NAME",
                Style::new().fg(TEXT).bg(SURFACE_RAISED),
            ),
            Span::styled(" ".repeat(100), Style::new().bg(SURFACE_RAISED)),
        ]))
        .styles(PaneStyles {
            background: Some(Style::new().bg(SURFACE_RAISED)),
            border: Style::new().fg(ACCENT).bg(SURFACE_RAISED),
            focused_border: Style::new().fg(ACCENT_STRONG).bg(SURFACE_RAISED),
        });
    let state = Cell::new(PaneState::new(area));
    paint_component(
        frame,
        area,
        &PaneComponent::new("dialog-pane", panel, &state, EmptyComponent),
    );
    let content = alias_editor_content_area(app.conversation_pane.area);
    frame.fill(
        LocalRect::terminal(content),
        " ",
        Style::new().bg(SURFACE_RAISED),
    );
    let input_state = RefCell::new(editor.input.clone());
    let mut input = TextInputBoxComponent::new(
        "sender-alias-input",
        bmux_tui_components::text_input::TextInputPolicy::chat_composer(),
        &input_state,
    )
    .placeholder("Type a local display name…")
    .policy(TextInputBoxPolicy::bare().focused(true).rows(1, Some(1)));
    if let Some(error) = editor.error.as_deref() {
        input = input.error(error);
    }
    frame.write_line_with_fallback_style(
        LocalRect::terminal(Rect::new(
            content.x,
            content.y.saturating_add(content.height.saturating_sub(1)),
            content.width,
            1,
        )),
        &Line::from_spans([Span::styled(
            "Enter saves  ·  Esc cancels",
            Style::new().fg(MUTED),
        )]),
        Style::new().fg(TEXT).bg(SURFACE_RAISED),
    );
    let input_area = Rect::new(content.x, content.y, content.width, 1);
    paint_component(frame, input_area, &input);
    editor.input = input_state.into_inner();
}

const HELP_ACTIONS: [Action; 7] = [
    Action::FocusNext,
    Action::MoveDown,
    Action::Activate,
    Action::Refresh,
    Action::Authenticate,
    Action::Help,
    Action::Quit,
];

fn help_title_line() -> Line {
    Line::from_spans(vec![Span::styled(
        "KEYBOARD SHORTCUTS",
        Style::new().fg(ACCENT_STRONG).add_modifier(Modifier::BOLD),
    )])
}

fn help_action_line(app: &App, action: Action) -> Option<Line> {
    let labels = app.bindings.labels_for(action).join(", ");
    if labels.is_empty() {
        return None;
    }
    Some(help_shortcut_line(&labels, action.label()))
}

fn help_shortcut_line(labels: &str, label: &str) -> Line {
    let padding = " ".repeat(16_usize.saturating_sub(Span::raw(labels).width()));
    Line::from_spans(vec![
        Span::styled(
            format!("{labels}{padding}"),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::new().fg(TEXT)),
    ])
}

struct HelpComponent<'a>(&'a App);

impl Component for HelpComponent<'_> {
    fn layout(&self, constraints: Constraints, cx: &mut LayoutCx) -> LayoutNode {
        cx.record_measurement();
        let width = HELP_ACTIONS
            .iter()
            .filter_map(|action| help_action_line(self.0, *action))
            .map(|line| line.width())
            .max()
            .unwrap_or(0)
            .max(help_title_line().width())
            .saturating_add(8);
        LayoutNode::leaf(
            LayoutId::new("help"),
            constraints.constrain(LogicalSize::new(
                u64::try_from(width).unwrap_or(u64::MAX),
                12,
            )),
        )
    }

    fn paint(&self, layout: &LayoutNode, cx: &mut PaintCx<'_, '_>) {
        render_help(
            self.0,
            cx,
            Rect::new(
                0,
                0,
                u16::try_from(layout.size.width).unwrap_or(u16::MAX),
                u16::try_from(layout.size.height).unwrap_or(u16::MAX),
            ),
        );
    }
}

fn render_help(app: &App, cx: &mut PaintCx<'_, '_>, area: Rect) {
    let overlay = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2).min(10),
    );
    if overlay.is_empty() {
        return;
    }
    cx.fill(
        LocalRect::terminal(overlay),
        " ",
        Style::new().bg(SURFACE_RAISED),
    );
    cx.write_line_with_fallback_style(
        LocalRect::terminal(Rect::new(
            overlay.x.saturating_add(2),
            overlay.y,
            overlay.width.saturating_sub(4),
            1,
        )),
        &help_title_line(),
        Style::new().fg(TEXT).bg(SURFACE_RAISED),
    );
    for (index, action) in HELP_ACTIONS.into_iter().enumerate() {
        let Ok(index) = u16::try_from(index) else {
            break;
        };
        if index.saturating_add(2) >= overlay.height {
            break;
        }
        let Some(line) = help_action_line(app, action) else {
            continue;
        };
        cx.write_line_with_fallback_style(
            LocalRect::terminal(Rect::new(
                overlay.x.saturating_add(2),
                overlay.y.saturating_add(2).saturating_add(index),
                overlay.width.saturating_sub(4),
                1,
            )),
            &line,
            Style::new().fg(TEXT).bg(SURFACE_RAISED),
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
    render(app, &mut PaintCx::new(&mut frame));
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
    fn narrow_layout_keeps_sidebar_within_available_width() {
        for width in 20..=22 {
            let mut app = App::new(KeybindingRegistry::default());
            let area = Rect::new(3, 2, width, 10);
            render_to_buffer(&mut app, area);
            assert_eq!(app.space_pane.area.x, area.x);
            assert_eq!(app.space_pane.area.width, width);
            assert_eq!(app.conversation_pane.area.width, 0);
            assert_eq!(app.conversation_pane.area.x, area.x + width);
        }
    }

    #[test]
    fn small_terminal_fallback_is_local_opaque_and_does_not_wrap() {
        let mut app = App::new(KeybindingRegistry::default());
        let area = Rect::new(3, 2, 10, 4);
        let (buffer, hits) = render_to_buffer_and_hits(&mut app, area);
        let first_row: String = (3..13)
            .map(|x| buffer.get(Point::new(x, 2)).unwrap().symbol.as_str())
            .collect();
        assert_eq!(first_row, "Terminal i");
        for y in 2..6 {
            for x in 3..13 {
                let cell = buffer.get(Point::new(x, y)).unwrap();
                assert_eq!(cell.style.bg, Some(CANVAS));
                if y > 2 {
                    assert_eq!(cell.symbol, " ");
                }
            }
        }
        assert!(hits.regions().is_empty());
    }

    #[test]
    fn header_preferred_width_fits_short_and_long_statuses() {
        let mut app = App::new(KeybindingRegistry::default());
        for phase in [Phase::Ready, Phase::RecoverableError] {
            app.product.phase = phase;
            let expected_width = u16::try_from(
                Line::from_spans([Span::raw("gchatui  /  terminal conversations")])
                    .width()
                    .max(Line::from_spans([Span::raw(format!("● {}", status_text(&app)))]).width())
                    + 2,
            )
            .unwrap();
            let header = HeaderComponent(&app);
            let layout =
                header.layout(Constraints::loose(Size::new(100, 10)), &mut LayoutCx::new());
            assert_eq!(layout.size, LogicalSize::new(u64::from(expected_width), 2));
            let mut buffer = Buffer::empty(Rect::new(0, 0, expected_width, 2));
            let mut frame = Frame::new(&mut buffer);
            header.paint(&layout, &mut PaintCx::new(&mut frame));
            drop(frame);
            let status: String = (0..expected_width)
                .map(|x| buffer.get(Point::new(x, 1)).unwrap().symbol.as_str())
                .collect();
            assert_eq!(status.trim(), format!("● {}", status_text(&app)));
        }
    }

    #[test]
    fn help_shortcut_column_uses_display_width_for_wide_keys() {
        let line = help_shortcut_line("界", Action::Help.label());
        assert_eq!(line.spans[0].width(), 16);
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 1));
        let mut frame = Frame::new(&mut buffer);
        PaintCx::new(&mut frame).write_line_with_fallback_style(
            LocalRect::new(0, 0, 40, 1),
            &line,
            Style::new(),
        );
        drop(frame);
        assert_eq!(buffer.get(Point::new(0, 0)).unwrap().symbol, "界");
        assert_eq!(
            buffer.get(Point::new(16, 0)).unwrap().symbol,
            Action::Help.label().chars().next().unwrap().to_string(),
        );
    }

    #[test]
    fn help_without_bound_actions_measures_only_its_title() {
        let overrides = crate::keybind::KeybindingOverrides {
            unbind: HELP_ACTIONS.to_vec(),
            ..Default::default()
        };
        let app = App::new(KeybindingRegistry::with_overrides(&overrides).unwrap());
        let help = HelpComponent(&app);
        let layout = help.layout(Constraints::loose(Size::new(100, 20)), &mut LayoutCx::new());
        assert_eq!(layout.size, LogicalSize::new(26, 12));
        for action in HELP_ACTIONS {
            assert!(help_action_line(&app, action).is_none());
        }
        let mut buffer = Buffer::empty(Rect::new(0, 0, 26, 12));
        let mut frame = Frame::new(&mut buffer);
        help.paint(&layout, &mut PaintCx::new(&mut frame));
        drop(frame);
        assert_eq!(buffer.get(Point::new(4, 1)).unwrap().symbol, "K");
        for y in 3..11 {
            for x in 4..21 {
                assert_eq!(buffer.get(Point::new(x, y)).unwrap().symbol, " ");
            }
        }
    }

    #[test]
    fn help_with_empty_inset_does_not_paint_into_parent() {
        let app = App::new(KeybindingRegistry::default());
        for area in [
            Rect::new(0, 0, 30, 0),
            Rect::new(0, 0, 30, 1),
            Rect::new(0, 0, 30, 2),
            Rect::new(0, 0, 4, 10),
        ] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 12));
            let expected = buffer.clone();
            let mut frame = Frame::new(&mut buffer);
            render_help(&app, &mut PaintCx::new(&mut frame), area);
            drop(frame);
            assert_eq!(buffer, expected);
        }
    }

    #[test]
    fn header_paint_respects_allocated_height_inside_a_larger_clip() {
        let app = App::new(KeybindingRegistry::default());
        for height in 0..=2 {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 4));
            let mut frame = Frame::new(&mut buffer);
            let header = HeaderComponent(&app);
            let layout = header.layout(
                Constraints::tight(Size::new(60, height)),
                &mut LayoutCx::new(),
            );
            header.paint(&layout, &mut PaintCx::new(&mut frame));
            drop(frame);
            for y in height..4 {
                for x in 0..60 {
                    let cell = buffer.get(Point::new(x, y)).unwrap();
                    assert_eq!(cell.symbol, " ");
                    assert_eq!(cell.style, Style::new());
                }
            }
            if height > 0 {
                assert_eq!(buffer.get(Point::new(1, 0)).unwrap().symbol, "g");
            }
            if height > 1 {
                assert_eq!(buffer.get(Point::new(1, 1)).unwrap().symbol, "●");
            }
        }
    }

    #[test]
    fn header_paint_is_translated_and_clipped_to_its_child() {
        let app = App::new(KeybindingRegistry::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 5));
        let mut frame = Frame::new(&mut buffer);
        let header = HeaderComponent(&app);
        let layout = header.layout(Constraints::tight(Size::new(12, 2)), &mut LayoutCx::new());
        assert_eq!(layout.size, LogicalSize::new(12, 2));
        PaintCx::new(&mut frame).with_child(3, 1, LocalRect::new(0, 0, 4, 1), |cx| {
            header.paint(&layout, cx);
        });
        drop(frame);
        assert_eq!(buffer.get(Point::new(4, 1)).unwrap().symbol, "g");
        assert_eq!(buffer.get(Point::new(5, 1)).unwrap().symbol, "c");
        assert_eq!(buffer.get(Point::new(6, 1)).unwrap().symbol, "h");
        for y in 0..5 {
            for x in 0..20 {
                if y != 1 || !(3..7).contains(&x) {
                    let cell = buffer.get(Point::new(x, y)).unwrap();
                    assert_eq!(cell.symbol, " ");
                    assert_eq!(cell.style, Style::new());
                }
            }
        }
    }

    #[test]
    fn help_paint_is_translated_and_clipped_to_its_child() {
        let app = App::new(KeybindingRegistry::default());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 30, 12));
        let mut frame = Frame::new(&mut buffer);
        let help = HelpComponent(&app);
        let layout = help.layout(Constraints::tight(Size::new(24, 10)), &mut LayoutCx::new());
        assert_eq!(layout.size, LogicalSize::new(24, 10));
        PaintCx::new(&mut frame).with_child(3, 2, LocalRect::new(0, 0, 7, 2), |cx| {
            help.paint(&layout, cx);
        });
        drop(frame);
        assert_eq!(buffer.get(Point::new(7, 3)).unwrap().symbol, "K");
        assert_eq!(buffer.get(Point::new(8, 3)).unwrap().symbol, "E");
        assert_eq!(buffer.get(Point::new(9, 3)).unwrap().symbol, "Y");
        for y in 0..12 {
            for x in 0..30 {
                if y != 3 || !(5..10).contains(&x) {
                    let cell = buffer.get(Point::new(x, y)).unwrap();
                    assert_eq!(cell.symbol, " ");
                    assert_eq!(cell.style, Style::new());
                }
            }
        }
    }

    #[test]
    fn auth_menu_routes_registry_actions_and_cancel_without_credentials() {
        let mut app = App::new(KeybindingRegistry::default());
        app.apply_action(Action::Authenticate);
        assert!(app.auth_menu);
        let down = "Down".parse::<crate::keybind::KeyChord>().unwrap().stroke();
        app.handle_auth_menu(&Event::Key(down));
        assert_eq!(app.auth_selection, 1);
        let activate = "Enter"
            .parse::<crate::keybind::KeyChord>()
            .unwrap()
            .stroke();
        let update = app.handle_auth_menu(&Event::Key(activate));
        assert!(matches!(update.lifecycle, Lifecycle::Exit));
        assert_eq!(*app.auth_result.lock().unwrap(), Some(1));
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
            rich_content: Vec::new(),
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
        for (index, cell) in buffer.cells().iter().enumerate() {
            assert!(
                cell.style
                    .bg
                    .is_some_and(|background| background != Color::Default),
                "cell ({}, {}) {:?} used the transparent terminal background",
                index % 100,
                index / 100,
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
                rich_content: Vec::new(),
            });
        }
        app.product.phase = Phase::Ready;
        app.rebuild_projections();
        app.follow_conversation_bottom = true;
        let _buffer = render_to_buffer(&mut app, Rect::new(0, 0, 100, 24));
        assert!(app.conversation_view.scroll.vertical_offset() > 0);
        let left_before = app.spaces.vertical_scroll();
        let right_before = app.conversation_view.scroll.vertical_offset();

        app.focused_pane = FocusedPane::Conversation;
        let up = "k".parse::<crate::keybind::KeyChord>().unwrap();
        let _ = app.update_terminal(Event::Key(up.stroke()));
        assert_eq!(app.spaces.vertical_scroll(), left_before);
        assert!(app.conversation_view.scroll.vertical_offset() < right_before);

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
    fn rich_preview_opens_with_mouse_and_keyboard_and_restores_scroll() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.phase = Phase::Ready;
        app.product.messages = vec![crate::model::Message {
            id: crate::model::MessageId("synthetic-rich".into()),
            thread_id: None,
            sender: None,
            text: "Example".into(),
            create_time: String::new(),
            is_thread_reply: false,
            unsupported_content: false,
            rich_content: vec![crate::model::RichContent {
                title: "Example card".into(),
                text: "<b>Full content</b>".into(),
                image_url: None,
                links: Vec::new(),
            }],
        }];
        app.rebuild_projections();
        let (_, hits) = render_to_buffer_and_hits(&mut app, Rect::new(0, 0, 100, 30));
        app.interactions.commit_scene(hits, None);
        let area = conversation_content_area(app.conversation_pane.area);
        let row = app
            .conversation_view
            .item_offset(&app.rich_items[0].0)
            .unwrap();
        let point = Point::new(area.x + 1, area.y + u16::try_from(row).unwrap());
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.update_terminal(Event::Mouse(MouseEvent::new(kind, point)));
        }
        assert!(app.viewer.is_some());
        let before = app.conversation_view.scroll.vertical_offset();
        let cancel = app.bindings.labels_for(Action::Cancel)[0]
            .parse::<crate::keybind::KeyChord>()
            .unwrap()
            .stroke();
        app.update_terminal(Event::Key(cancel));
        assert!(app.viewer.is_none());
        assert_eq!(app.conversation_view.scroll.vertical_offset(), before);
        app.focused_pane = FocusedPane::Conversation;
        let activate = app.bindings.labels_for(Action::Activate)[0]
            .parse::<crate::keybind::KeyChord>()
            .unwrap()
            .stroke();
        app.update_terminal(Event::Key(activate));
        assert!(app.viewer.is_some());
        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 100, 30));
        assert!(buffer.cells().iter().any(|cell| cell.symbol == "F"));
    }

    #[test]
    fn stale_image_generation_cannot_replace_current_media() {
        let mut app = App::new(KeybindingRegistry::default());
        app.image_generation = 2;
        let mut images = crate::rich_content::Images::new();
        images.insert(
            "https://example.com/synthetic.png".into(),
            Err(crate::rich_content::ImageError::Network),
        );
        app.update(RuntimeEvent::Message(AppMessage::ImagesLoaded {
            generation: 1,
            space: None,
            images,
        }))
        .unwrap();
        assert!(app.images.is_empty());
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
            rich_content: Vec::new(),
            },
            crate::model::Message {
                id: crate::model::MessageId("ordinary".to_string()),
                thread_id: None,
                sender: None,
                text: "ordinary".to_string(),
                create_time: "09:05".to_string(),
                is_thread_reply: false,
                unsupported_content: false,
            rich_content: Vec::new(),
            },
            crate::model::Message {
                id: crate::model::MessageId("reply".to_string()),
                thread_id: Some(crate::model::ThreadId("thread".to_string())),
                sender: None,
                text: "reply".to_string(),
                create_time: "09:10".to_string(),
                is_thread_reply: true,
                unsupported_content: false,
            rich_content: Vec::new(),
            },
        ];
        app.product.phase = Phase::Ready;
        app.rebuild_projections();
        app.conversation_view.scroll.set_vertical_offset(u64::MAX);
        let (_buffer, hits) = render_to_buffer_and_hits(&mut app, Rect::new(0, 0, 70, 16));
        app.interactions.commit_scene(hits, None);
        let link = app.thread_activity_links.first().unwrap();
        let area = conversation_content_area(app.conversation_pane.area);
        let hit_area = thread_activity_area(link, &app.conversation_view, area).unwrap();
        let point = Point::new(hit_area.x.saturating_add(2), hit_area.y);
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Down(MouseButton::Left),
            point,
        )));
        let _ = app.update_terminal(Event::Mouse(MouseEvent::new(
            MouseEventKind::Up(MouseButton::Left),
            point,
        )));
        assert_eq!(app.conversation_view.scroll.vertical_offset(), 0);
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
                rich_content: Vec::new(),
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
    fn message_background_extends_across_the_conversation_width() {
        let mut app = App::new(KeybindingRegistry::default());
        app.product.messages = vec![crate::model::Message {
            id: crate::model::MessageId("messages/example".to_string()),
            thread_id: None,
            sender: None,
            text: "Synthetic message".to_string(),
            create_time: "10:42".to_string(),
            is_thread_reply: false,
            unsupported_content: false,
            rich_content: Vec::new(),
        }];
        app.rebuild_projections();

        let buffer = render_to_buffer(&mut app, Rect::new(0, 0, 100, 26));
        let area = conversation_content_area(app.conversation_pane.area);
        let rightmost = buffer
            .get(Point::new(area.right().saturating_sub(1), area.y))
            .expect("message row should be inside the rendered buffer");
        assert_eq!(rightmost.style.bg, Some(MESSAGE_BG));
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
            rich_content: Vec::new(),
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
