//! The tests the native backend cannot pass yet, each with the tickets
//! (42: more spaces, floats, finder, worktrees, roots) whose part of the
//! backend it needs; 41 and 43 have landed. A ticket that lands takes its
//! number off every entry, and deletes the entries left with none.
//!
//! A native run skips them: `scripts/test-native.sh` turns each into a
//! `--skip` (libtest matches it as a substring of the test's name, in
//! every test binary; `tests/native_server.rs` checks no name here
//! matches a test not listed). A listed test run on native anyway fails
//! at once, naming its tickets ([`check`]).

/// `file::test` (the file under `tests/`), and the tickets it needs.
pub const NATIVE_NEEDS: &[(&str, &[&str])] = &[
    (
        "attention::c_a_a_cycles_through_sessions_that_need_you",
        &["42"],
    ),
    (
        "close_fork::x_on_a_space_with_a_waiting_claude_asks_then_closes_the_space",
        &["42"],
    ),
    (
        "close_fork::x_on_a_quiet_space_closes_it_without_asking",
        &["42"],
    ),
    (
        "close_fork::f_on_a_space_forks_its_agent_into_a_new_tab_of_that_space",
        &["42"],
    ),
    ("copy_mode::copy_mode_works_in_the_float_shown", &["42"]),
    (
        "finder::g_opens_the_finder_and_typing_a_tabs_name_then_enter_shows_it",
        &["42"],
    ),
    ("finder::an_agent_row_opens_its_pane", &["42"]),
    (
        "finder::state_letters_filter_agents_on_an_empty_query",
        &["42"],
    ),
    (
        "finder::esc_and_a_click_outside_cancel_and_change_nothing",
        &["42"],
    ),
    ("finder::a_click_on_a_row_opens_it", &["42"]),
    (
        "finder::the_finder_opened_from_the_dashboard_returns_to_it_on_esc",
        &["42"],
    ),
    (
        "floats::the_key_shows_a_float_over_everything_and_hides_it_with_its_program_kept",
        &["42"],
    ),
    (
        "floats::keys_go_to_the_float_and_not_to_the_terminal_under_it",
        &["42"],
    ),
    (
        "floats::when_its_program_exits_the_float_closes_and_the_next_toggle_starts_it_again",
        &["42"],
    ),
    (
        "floats::a_command_that_changes_the_page_hides_the_float_and_tabs_never_include_it",
        &["42"],
    ),
    ("floats::each_space_has_its_own_float", &["42"]),
    (
        "floats::closing_the_last_tab_closes_its_space_with_the_float",
        &["42"],
    ),
    ("floats::pasted_text_goes_to_the_float", &["42"]),
    ("floats::a_click_outside_the_float_does_nothing", &["42"]),
    ("floats::claude_in_a_float_is_not_followed", &["42"]),
    ("floats::a_float_on_the_semicolon_key_opens", &["42"]),
    (
        "floats::a_program_that_exits_at_once_gives_a_notice_and_never_a_tab",
        &["42"],
    ),
    (
        "floats::hooks_from_a_float_that_has_closed_are_still_left_out",
        &["42"],
    ),
    (
        "floats::a_float_whose_key_left_the_config_is_closed_at_the_next_start",
        &["42"],
    ),
    ("floats::the_tab_under_a_float_is_not_looked_at", &["42"]),
    (
        "floats::clicks_and_the_wheel_inside_the_float_reach_its_pane",
        &["42"],
    ),
    (
        "floats::the_finder_opens_over_a_float_and_cancelling_leaves_it_shown",
        &["42"],
    ),
    (
        "keys::a_remapped_sidebar_key_works_and_its_old_key_does_not",
        &["42"],
    ),
    ("keys::ctrl_does_not_matter_for_sidebar_keys", &["42"]),
    ("keys::in_a_float_a_direct_chord_goes_to_the_float", &["42"]),
    ("mouse::clicking_a_space_shows_its_tabs", &["42"]),
    (
        "prompt::a_new_space_asks_for_its_name_and_enter_keeps_the_default",
        &["42"],
    ),
    (
        "prompt::a_new_space_takes_the_name_typed_and_esc_makes_none",
        &["42"],
    ),
    (
        "prompt::a_new_space_with_a_taken_name_shows_a_notice_after_the_popup_closes",
        &["42"],
    ),
    (
        "prompt::naming_a_new_space_from_the_sidebar_keeps_the_sidebar_focused",
        &["42"],
    ),
    (
        "prompt::naming_a_new_space_from_the_dashboard_keeps_the_dashboard_open",
        &["42"],
    ),
    (
        "sessions::agents_of_every_space_are_listed_with_their_space_and_tab",
        &["42"],
    ),
    (
        "sessions::c_a_w_selects_the_current_space_and_esc_leaves_without_switching",
        &["42"],
    ),
    (
        "sessions::enter_on_an_agent_jumps_to_its_space_tab_and_pane",
        &["42"],
    ),
    (
        "sessions::c_a_s_starts_on_the_current_space_when_no_agent_is_focused",
        &["42"],
    ),
    ("sessions::clicking_an_agent_row_jumps_to_its_pane", &["42"]),
    ("sessions::x_on_an_agent_row_closes_its_pane", &["42"]),
    ("sessions::dragging_the_border_resizes_the_sidebar", &["42"]),
    (
        "sessions::a_click_on_the_border_without_a_drag_is_a_click_on_the_row",
        &["42"],
    ),
    (
        "space_root::the_first_space_and_c_a_c_take_the_focused_directory_as_root",
        &["42"],
    ),
    (
        "space_root::a_worktree_space_s_root_is_its_worktree_from_the_keys_and_the_command",
        &["42"],
    ),
    (
        "space_root::by_default_new_tabs_and_splits_start_in_the_focused_directory",
        &["42"],
    ),
    (
        "space_root::with_root_new_tabs_and_splits_start_in_the_space_s_root",
        &["42"],
    ),
    (
        "space_root::c_a_r_makes_the_focused_directory_the_root_with_a_notice",
        &["42"],
    ),
    (
        "space_root::the_finder_shows_a_worktree_space_s_root_with_its_branch",
        &["42"],
    ),
    (
        "tabs::capital_c_opens_a_new_space_with_its_own_tabs",
        &["42"],
    ),
    ("tabs::picking_a_space_shows_its_tabs", &["42"]),
    ("tabs::the_current_space_has_its_own_background", &["42"]),
    (
        "tabs::renaming_a_space_to_a_taken_name_shows_an_error_and_keeps_the_old_name",
        &["42"],
    ),
    ("tabs::closing_a_spaces_last_tab_closes_the_space", &["42"]),
    (
        "tabs::reattaching_shows_every_space_and_tab_as_the_backend_has_them",
        &["42"],
    ),
    (
        "tabs::c_a_tab_goes_back_to_the_last_page_across_spaces",
        &["42"],
    ),
    (
        "tabs::c_a_tab_skips_a_closed_page_for_an_older_one",
        &["42"],
    ),
    (
        "worktree_cli::new_makes_a_worktree_and_a_space_that_a_running_paddock_shows",
        &["42"],
    ),
    (
        "worktree_cli::an_existing_branch_is_checked_out_and_failures_make_nothing",
        &["42"],
    ),
    (
        "worktree_cli::list_tells_each_kind_of_worktree_apart",
        &["42"],
    ),
    (
        "worktree_cli::rm_never_deletes_the_tree_its_caller_or_a_busy_claude_is_in",
        &["42"],
    ),
    (
        "worktree_cli::rm_counts_a_busy_claude_in_a_float_and_list_leaves_floats_out",
        &["42"],
    ),
    (
        "worktrees::c_a_w_makes_a_worktree_and_a_space_in_it",
        &["42"],
    ),
    (
        "worktrees::an_existing_branch_is_checked_out_and_bad_names_or_git_errors_make_nothing",
        &["42"],
    ),
    (
        "worktrees::c_a_o_lists_worktrees_without_a_space_and_opens_one",
        &["42"],
    ),
    (
        "worktrees::x_on_a_clean_merged_worktree_space_deletes_it_after_one_yes",
        &["42"],
    ),
    (
        "worktrees::x_on_a_dirty_worktree_space_asks_again_naming_the_risk",
        &["42"],
    ),
    (
        "worktrees::a_dirty_unmerged_worktree_names_both_risks_and_the_branch_is_kept",
        &["42"],
    ),
    (
        "worktrees::commits_made_after_the_question_are_never_lost",
        &["42"],
    ),
    (
        "worktrees::every_space_in_the_worktree_closes_with_it",
        &["42"],
    ),
    (
        "worktrees::changes_made_after_the_question_are_never_forced_away",
        &["42"],
    ),
    (
        "worktrees::a_detached_worktree_with_its_own_commits_asks_again",
        &["42"],
    ),
    (
        "worktrees::x_on_a_worktree_space_paddock_did_not_make_only_closes_it",
        &["42"],
    ),
    ("worktrees::the_main_checkout_is_never_deleted", &["42"]),
];

/// On a native run, fail the running test at once if it is listed: it
/// needs tickets the native backend does not have yet.
pub fn check() {
    if !super::backend::native() {
        return;
    }
    // libtest names each test's thread after the test, with one test
    // thread too (`--test-threads=1`, Rust 1.93; checked by
    // `tests/native_server.rs`). Without a name this check cannot say,
    // and only the script's skips hold.
    let Some(test) = std::thread::current().name().map(str::to_owned) else {
        return;
    };
    if test == "main" {
        return;
    }
    let path = format!("{}::{test}", env!("CARGO_CRATE_NAME"));
    if let Some((_, tickets)) = NATIVE_NEEDS.iter().find(|(p, _)| *p == path) {
        panic!(
            "{path} needs ticket {} on the native backend: run the native suite with \
             scripts/test-native.sh, which skips it (tests/harness/native_skip.rs)",
            tickets.join(" and ")
        );
    }
}
