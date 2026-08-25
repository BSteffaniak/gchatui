# TUI component inventory

The initial interface uses existing bmux components rather than local widget
implementations.

| Product need | bmux component | Enabled feature |
| --- | --- | --- |
| Space navigation | `SelectableList` | `selectable-list` |
| Pane framing and pointer focus | `Pane` | `pane` |
| Key hints derived from active bindings | `KeyHintBar` | `key-hint-bar` |
| Runtime state/message row | `StatusBar` | `status-bar` |
| Conversation scrolling | `ScrollArea` / `TextView` | `scroll-area`, `text-view` |

The prototype has no gchatui-specific reusable UI primitive. Product-specific
composition remains in the application renderer. Any future local component needs
written evidence that existing bmux components cannot express its behavior;
generally reusable gaps should be addressed upstream.
