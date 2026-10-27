//! The tests the native backend cannot pass yet, each with the tickets
//! (42: more spaces, floats, finder, worktrees, roots; 43: agents and
//! hooks) whose part of the backend it needs; 41 (tabs, splits, zoom,
//! resizing) has landed. A ticket that lands takes its number off every entry, and
//! deletes the entries left with none.
//!
//! A native run skips them: `scripts/test-native.sh` turns each into a
//! `--skip` (libtest matches it as a substring of the test's name, in
//! every test binary; `tests/native_server.rs` checks no name here
//! matches a test not listed). A listed test run on native anyway fails
//! at once, naming its tickets ([`check`]).

/// `file::test` (the file under `tests/`), and the tickets it needs.
pub const NATIVE_NEEDS: &[(&str, &[&str])] = &[
    (
        "agy::an_agy_session_goes_idle_working_waiting_done_then_ends_at_the_shell",
        &["43"],
    ),
    ("agy::an_agy_session_ends_when_its_terminal_closes", &["43"]),
    (
        "agy::reports_from_other_programs_or_without_a_session_change_nothing",
        &["43"],
    ),
    ("agy::the_same_report_after_a_hook_shows_again", &["43"]),
    (
        "agy::a_report_right_after_the_start_hook_waits_for_its_session",
        &["43"],
    ),
    (
        "agy::a_report_just_before_its_session_starts_belongs_to_it_live_and_on_replay",
        &["43"],
    ),
    (
        "agy::reports_from_a_terminal_without_an_agy_session_are_not_recorded",
        &["43"],
    ),
    (
        "agy::agy_quitting_without_a_prompt_does_not_notify",
        &["43"],
    ),
    (
        "attention::waiting_and_finished_turns_notify_once_each",
        &["43"],
    ),
    (
        "attention::the_terminal_you_look_at_never_notifies",
        &["43"],
    ),
    (
        "attention::events_while_paddock_is_closed_never_notify",
        &["43"],
    ),
    (
        "attention::c_a_a_cycles_through_sessions_that_need_you",
        &["42", "43"],
    ),
    (
        "attention::c_a_a_focuses_the_split_running_the_session",
        &["43"],
    ),
    (
        "attention::a_turn_finished_out_of_sight_is_done_until_you_look_at_it",
        &["43"],
    ),
    (
        "attention::a_turn_finished_in_a_split_hidden_by_a_zoom_is_done_until_unzoomed",
        &["43"],
    ),
    (
        "attention::c_a_a_to_a_split_hidden_by_a_zoom_gives_the_layout_back_focused_there",
        &["43"],
    ),
    (
        "attention::a_turn_finished_in_view_is_idle_at_once",
        &["43"],
    ),
    (
        "attention::a_notifier_that_fails_says_so_in_a_notice",
        &["43"],
    ),
    (
        "attention::finished_turns_can_stop_notifying_while_waits_still_do",
        &["43"],
    ),
    (
        "attention::waits_can_stop_notifying_while_finished_turns_still_do",
        &["43"],
    ),
    (
        "attention::turning_waits_off_silences_a_subagents_wait_too",
        &["43"],
    ),
    (
        "close_fork::x_closes_an_idle_session_without_asking",
        &["43"],
    ),
    ("close_fork::x_asks_while_claude_is_running", &["43"]),
    (
        "close_fork::the_question_shows_even_with_many_tabs",
        &["43"],
    ),
    (
        "close_fork::x_on_a_space_with_a_waiting_claude_asks_then_closes_the_space",
        &["42", "43"],
    ),
    (
        "close_fork::x_on_a_quiet_space_closes_it_without_asking",
        &["42"],
    ),
    (
        "close_fork::f_forks_the_session_into_a_new_tab_in_its_directory",
        &["43"],
    ),
    (
        "close_fork::f_on_a_space_forks_its_agent_into_a_new_tab_of_that_space",
        &["42", "43"],
    ),
    (
        "close_fork::closing_the_only_pane_of_a_spaces_only_tab_asks_to_close_the_space",
        &["43"],
    ),
    (
        "close_fork::a_busy_close_asked_from_the_sidebar_gives_the_sidebar_back",
        &["43"],
    ),
    ("copy_mode::copy_mode_works_in_the_float_shown", &["42"]),
    ("dashboard::rows_are_tagged_with_their_agents_kind", &["43"]),
    (
        "dashboard::o_opens_the_dashboard_and_esc_closes_it",
        &["43"],
    ),
    (
        "dashboard::slash_filters_the_list_enter_keeps_the_filter_and_esc_clears_it",
        &["43"],
    ),
    (
        "dashboard::the_list_sits_above_the_preview_and_the_keys_at_the_bottom",
        &["43"],
    ),
    (
        "dashboard::j_and_k_move_and_enter_jumps_to_the_rows_terminal",
        &["43"],
    ),
    (
        "dashboard::d_dismisses_a_row_and_it_stays_gone_after_reopening",
        &["43"],
    ),
    (
        "dashboard::a_closed_terminals_session_leaves_and_the_selection_moves_to_the_next",
        &["43"],
    ),
    (
        "dashboard::a_session_whose_claude_quit_leaves_while_its_terminal_stays",
        &["43"],
    ),
    (
        "dashboard::the_preview_shows_the_selected_terminal_live",
        &["43"],
    ),
    (
        "dashboard::i_types_into_the_previewed_terminal_and_esc_is_never_sent",
        &["43"],
    ),
    (
        "dashboard::esc_and_a_key_in_one_read_leave_interactive_preview_unsent",
        &["43"],
    ),
    (
        "dashboard::x_asks_while_claude_runs_then_returns_to_the_dashboard",
        &["43"],
    ),
    (
        "dashboard::a_click_outside_the_close_question_cancels_it_back_to_the_dashboard",
        &["43"],
    ),
    ("dashboard::f_forks_the_selected_session", &["43"]),
    (
        "external::an_external_session_shows_dim_and_last_with_its_state_and_branch",
        &["43"],
    ),
    (
        "external::external_false_hides_the_group_and_its_notifications",
        &["43"],
    ),
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
        "gemini::a_gemini_session_goes_working_waiting_idle_then_ends",
        &["43"],
    ),
    ("gemini::fork_refuses_a_gemini_session", &["43"]),
    (
        "keys::a_remapped_sidebar_key_works_and_its_old_key_does_not",
        &["42"],
    ),
    ("keys::ctrl_does_not_matter_for_sidebar_keys", &["42"]),
    ("keys::in_a_float_a_direct_chord_goes_to_the_float", &["42"]),
    ("mouse::clicking_a_space_shows_its_tabs", &["42"]),
    (
        "plugin::the_plugin_hooks_alone_record_every_event_once",
        &["43"],
    ),
    (
        "plugin::the_plugin_and_settings_hooks_together_record_every_event_once",
        &["43"],
    ),
    (
        "plugin::a_hook_naming_claude_records_as_claudes_and_an_unknown_one_stays_silent",
        &["43"],
    ),
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
    ("sessions::hook_events_drive_the_agent_row", &["43"]),
    (
        "sessions::agents_of_every_space_are_listed_with_their_space_and_tab",
        &["42", "43"],
    ),
    (
        "sessions::events_recorded_while_paddock_is_closed_appear_on_reopen",
        &["43"],
    ),
    ("sessions::claude_agents_corrects_missed_events", &["43"]),
    (
        "sessions::c_a_w_selects_the_current_space_and_esc_leaves_without_switching",
        &["42"],
    ),
    (
        "sessions::enter_on_an_agent_jumps_to_its_space_tab_and_pane",
        &["42", "43"],
    ),
    (
        "sessions::c_a_s_starts_on_the_current_space_when_no_agent_is_focused",
        &["42", "43"],
    ),
    (
        "sessions::the_focused_agent_has_the_current_background",
        &["43"],
    ),
    (
        "sessions::c_a_s_starts_on_the_focused_agent_even_with_the_sidebar_hidden",
        &["43"],
    ),
    (
        "sessions::clicking_an_agent_row_jumps_to_its_pane",
        &["42", "43"],
    ),
    ("sessions::x_on_an_agent_row_closes_its_pane", &["42", "43"]),
    ("sessions::parallel_hooks_never_interleave_records", &["43"]),
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
        "tasks::switching_branches_adds_a_task_row_for_each_branch",
        &["43"],
    ),
    (
        "tasks::task_rows_show_ahead_behind_and_line_changes",
        &["43"],
    ),
    (
        "tasks::no_rows_for_the_default_branch_or_shells_without_claude",
        &["43"],
    ),
    (
        "tasks::parallel_subagents_show_as_task_rows_from_running_to_done",
        &["43"],
    ),
    (
        "tasks::subagent_work_done_while_paddock_is_closed_appears_on_reopen",
        &["43"],
    ),
    (
        "tasks::a_session_that_ends_while_paddock_is_closed_is_gone_with_its_tasks_on_reopen",
        &["43"],
    ),
    (
        "tasks::j_skips_branch_rows_and_d_dismisses_them_with_the_session",
        &["43"],
    ),
    (
        "tasks::d_on_a_session_row_hides_it_with_its_tasks_until_claude_reports_again",
        &["43"],
    ),
    (
        "tasks::a_closed_terminals_rows_leave_at_once_and_stay_gone_after_reopening",
        &["43"],
    ),
    (
        "tasks::a_session_whose_claude_quit_leaves_with_its_task_rows",
        &["43"],
    ),
    (
        "tasks::a_new_conversation_in_the_same_terminal_keeps_its_task_rows",
        &["43"],
    ),
    (
        "tasks::j_skips_subagent_rows_and_d_on_the_session_takes_them_along",
        &["43"],
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
