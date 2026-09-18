//! Every bindable action.
//!
//! Action names used in the `[keys]` config section are the snake_case form
//! of the variant name, e.g. `NextHeading` → `next_heading`.

/// A user-triggerable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    /// Close the focused document; the last one quits (`q`, `Ctrl-C`).
    Quit,
    /// Cancel the current sub-mode (`Esc`).
    Cancel,
    /// Scroll one line down (`j`, `↓`).
    ScrollDown,
    /// Scroll one line up (`k`, `↑`).
    ScrollUp,
    /// Page down (`PgDn`, `Space`).
    PageDown,
    /// Page up (`PgUp`, `b`).
    PageUp,
    /// Half page down (`Ctrl-D`).
    HalfPageDown,
    /// Half page up (`Ctrl-U`).
    HalfPageUp,
    /// Horizontal scroll left (`h`, `←`).
    ScrollLeft,
    /// Horizontal scroll right (`l`, `→`).
    ScrollRight,
    /// Jump to the top (`g`).
    Top,
    /// Jump to the bottom (`G`).
    Bottom,
    /// Open the search prompt (`/`).
    Search,
    /// Next search result (`n`).
    NextSearch,
    /// Previous search result (`N`).
    PreviousSearch,
    /// Next heading (`]`).
    NextHeading,
    /// Previous heading (`[`).
    PreviousHeading,
    /// Next heading at same or higher level (`}`).
    NextHeadingSameLevel,
    /// Previous heading at same or higher level (`{`).
    PreviousHeadingSameLevel,
    /// Toggle the table of contents (`t`).
    ToggleToc,
    /// Toggle the key hints sidebar (`K`).
    ToggleKeyHints,
    /// Toggle mouse reporting, handing the mouse back to the terminal so
    /// text can be selected with it (`m`).
    ToggleMouse,
    /// Move the focus to the other pane of a split (`Ctrl-W`).
    FocusOtherPane,
    /// Show the next tab (`Ctrl-N`).
    NextTab,
    /// Show the previous tab (`Ctrl-P`).
    PreviousTab,
    /// Activate selected heading/link (`Enter`).
    Activate,
    /// Open the selected link (`o`).
    OpenLink,
    /// Select next link (`Tab`).
    NextLink,
    /// Select previous link (`Shift-Tab`).
    PreviousLink,
    /// Toggle current section fold (`za`).
    ToggleFold,
    /// Collapse current section (`zc`).
    CollapseFold,
    /// Expand current section (`zo`).
    ExpandFold,
    /// Collapse all sections (`zM`).
    CollapseAll,
    /// Expand all sections (`zR`).
    ExpandAll,
    /// Show the help overlay (`?`).
    Help,
    /// Open the `:` command line (`:`).
    CommandPrompt,
    /// Toggle Mermaid source view on a diagram (`s`).
    ToggleMermaidSource,
    /// Move to the enclosing node (`H`).
    ///
    /// The structured-document counterpart of "go up a level": from a
    /// mapping entry to the mapping, from a Markdown body node to its
    /// heading, from a heading to its parent's.
    ParentNode,
    /// Move to the first node inside this one (`L`).
    FirstChild,
}

impl Action {
    /// All actions, in help-display order.
    pub const ALL: &'static [Action] = &[
        Action::Quit,
        Action::Cancel,
        Action::ScrollDown,
        Action::ScrollUp,
        Action::PageDown,
        Action::PageUp,
        Action::HalfPageDown,
        Action::HalfPageUp,
        Action::ScrollLeft,
        Action::ScrollRight,
        Action::Top,
        Action::Bottom,
        Action::Search,
        Action::NextSearch,
        Action::PreviousSearch,
        Action::NextHeading,
        Action::PreviousHeading,
        Action::NextHeadingSameLevel,
        Action::PreviousHeadingSameLevel,
        Action::ToggleToc,
        Action::ToggleKeyHints,
        Action::ToggleMouse,
        Action::FocusOtherPane,
        Action::NextTab,
        Action::PreviousTab,
        Action::Activate,
        Action::OpenLink,
        Action::NextLink,
        Action::PreviousLink,
        Action::ToggleFold,
        Action::CollapseFold,
        Action::ExpandFold,
        Action::CollapseAll,
        Action::ExpandAll,
        Action::Help,
        Action::CommandPrompt,
        Action::ToggleMermaidSource,
        Action::ParentNode,
        Action::FirstChild,
    ];

    /// snake_case name used in configuration.
    pub fn name(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Cancel => "cancel",
            Action::ScrollDown => "scroll_down",
            Action::ScrollUp => "scroll_up",
            Action::PageDown => "page_down",
            Action::PageUp => "page_up",
            Action::HalfPageDown => "half_page_down",
            Action::HalfPageUp => "half_page_up",
            Action::ScrollLeft => "scroll_left",
            Action::ScrollRight => "scroll_right",
            Action::Top => "top",
            Action::Bottom => "bottom",
            Action::Search => "search",
            Action::NextSearch => "next_search",
            Action::PreviousSearch => "previous_search",
            Action::NextHeading => "next_heading",
            Action::PreviousHeading => "previous_heading",
            Action::NextHeadingSameLevel => "next_heading_same_level",
            Action::PreviousHeadingSameLevel => "previous_heading_same_level",
            Action::ToggleToc => "toggle_toc",
            Action::ToggleKeyHints => "toggle_key_hints",
            Action::ToggleMouse => "toggle_mouse",
            Action::FocusOtherPane => "focus_other_pane",
            Action::NextTab => "next_tab",
            Action::PreviousTab => "previous_tab",
            Action::Activate => "activate",
            Action::OpenLink => "open_link",
            Action::NextLink => "next_link",
            Action::PreviousLink => "previous_link",
            Action::ToggleFold => "toggle_fold",
            Action::CollapseFold => "collapse_fold",
            Action::ExpandFold => "expand_fold",
            Action::CollapseAll => "collapse_all",
            Action::ExpandAll => "expand_all",
            Action::Help => "help",
            Action::CommandPrompt => "command_prompt",
            Action::ToggleMermaidSource => "toggle_mermaid_source",
            Action::ParentNode => "parent_node",
            Action::FirstChild => "first_child",
        }
    }

    /// Look up an action by its snake_case configuration name.
    pub fn from_name(name: &str) -> Option<Action> {
        Action::ALL.iter().copied().find(|a| a.name() == name)
    }

    /// Human-readable description for the help overlay.
    pub fn description(self) -> &'static str {
        match self {
            Action::Quit => "close document (the last one quits)",
            Action::Cancel => "cancel / close",
            Action::ScrollDown => "scroll down",
            Action::ScrollUp => "scroll up",
            Action::PageDown => "page down",
            Action::PageUp => "page up",
            Action::HalfPageDown => "half page down",
            Action::HalfPageUp => "half page up",
            Action::ScrollLeft => "scroll left",
            Action::ScrollRight => "scroll right",
            Action::Top => "top of document",
            Action::Bottom => "bottom of document",
            Action::Search => "search",
            Action::NextSearch => "next search result",
            Action::PreviousSearch => "previous search result",
            Action::NextHeading => "next heading / structural node",
            Action::PreviousHeading => "previous heading / structural node",
            Action::NextHeadingSameLevel => "next sibling (same or higher level)",
            Action::PreviousHeadingSameLevel => "previous sibling (same or higher level)",
            Action::ToggleToc => "toggle the outline (table of contents)",
            Action::ToggleKeyHints => "toggle key hints sidebar",
            Action::ToggleMouse => "toggle mouse (off: select text)",
            Action::FocusOtherPane => "focus the other pane",
            Action::NextTab => "next tab",
            Action::PreviousTab => "previous tab",
            Action::Activate => "fold at the cursor, or follow a link",
            Action::OpenLink => "open selected link",
            Action::NextLink => "select next link",
            Action::PreviousLink => "select previous link",
            Action::ToggleFold => "toggle the section or container at the cursor",
            Action::CollapseFold => "collapse the section or container",
            Action::ExpandFold => "expand the section or container",
            Action::CollapseAll => "collapse everything foldable",
            Action::ExpandAll => "expand everything foldable",
            Action::Help => "help",
            Action::CommandPrompt => "command line (: set a setting)",
            Action::ToggleMermaidSource => "toggle mermaid source",
            Action::ParentNode => "move out to the enclosing node",
            Action::FirstChild => "move in to the first node inside",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for &a in Action::ALL {
            assert_eq!(Action::from_name(a.name()), Some(a), "{a:?}");
        }
        assert_eq!(Action::from_name("nope"), None);
        assert_eq!(Action::from_name("next_heading"), Some(Action::NextHeading));
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = Action::ALL.iter().map(|a| a.name()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
    }
}
