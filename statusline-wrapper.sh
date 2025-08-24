#!/bin/bash
# Claude Code Statusline Wrapper
# Validates and passes Claude's JSON (already in snake_case) to the statusline binary

json_input=$(cat)

if command -v jq &> /dev/null; then
    # Validate and ensure all fields are present
    echo "$json_input" | jq '{
        session_id: .session_id,
        transcript_path: .transcript_path,
        workspace: {
            current_dir: (.workspace.current_dir // .cwd // null)
        },
        model: {
            display_name: .model.display_name
        },
        cost: (if .cost then {
            total_cost_usd: .cost.total_cost_usd,
            total_lines_added: .cost.total_lines_added,
            total_lines_removed: .cost.total_lines_removed
        } else null end)
    }' | ~/.local/bin/statusline
else
    # Fallback: pass through directly (jq recommended for validation)
    echo "$json_input" | ~/.local/bin/statusline
fi