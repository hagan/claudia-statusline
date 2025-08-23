#!/bin/bash
# Claude Code Statusline Wrapper
# This script adapts the statusline binary for Claude Code's JSON format
# 
# Claude Code passes JSON with this structure:
# {
#   "sessionId": "...",
#   "transcriptPath": "...",
#   "workspace": { "currentDir": "..." },
#   "model": { "displayName": "..." },
#   ...additional fields...
# }
#
# We need to transform it to our statusline's expected format:
# {
#   "session_id": "...",
#   "transcript_path": "...",
#   "workspace": { "current_dir": "..." },
#   "model": { "display_name": "..." }
# }

# Read JSON from stdin
json_input=$(cat)

# Check if jq is available for JSON transformation
if command -v jq &> /dev/null; then
    # Transform Claude Code's camelCase to our snake_case format
    transformed_json=$(echo "$json_input" | jq '{
        session_id: .sessionId,
        transcript_path: .transcriptPath,
        workspace: {
            current_dir: .workspace.currentDir
        },
        model: {
            display_name: .model.displayName
        }
    }')
    
    # Pass transformed JSON to statusline
    echo "$transformed_json" | ~/.local/bin/statusline
else
    # Fallback: try basic sed transformation (less reliable but works for simple cases)
    transformed_json=$(echo "$json_input" | sed \
        -e 's/"sessionId"/"session_id"/g' \
        -e 's/"transcriptPath"/"transcript_path"/g' \
        -e 's/"currentDir"/"current_dir"/g' \
        -e 's/"displayName"/"display_name"/g')
    
    echo "$transformed_json" | ~/.local/bin/statusline
fi

# Exit with success
exit 0