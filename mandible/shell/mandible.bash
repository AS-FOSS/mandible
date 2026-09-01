# mandible shell integration for bash.
#
#   eval "$(mandible --shell-init bash)"     # in ~/.bashrc
#
# Type a tool name, press Ctrl-X m, browse, press Enter: the command you
# selected replaces the line, ready to edit. Quit with q or Ctrl-C and the
# line is left exactly as it was.
#
# Requires bash 4.0 or newer: READLINE_LINE and READLINE_POINT are how a
# `bind -x` function edits the line being typed, and they arrived in 4.0.

_mandible_widget() {
  local tool selection
  # The first word of the line names the tool to open. Whatever else is
  # already typed is replaced by what comes back, which is the point: this
  # hands you a command to edit, not a fragment to splice into one.
  tool=${READLINE_LINE%%[[:space:]]*}
  [[ -n $tool ]] || return 0
  # `</dev/tty` because a widget can run with stdin somewhere else. The UI
  # draws on stderr, so the only thing arriving here is the selection --
  # and nothing arrives at all if the user quit instead of choosing.
  selection=$(mandible --print-selection -- "$tool" </dev/tty) || return 0
  [[ -n $selection ]] || return 0
  READLINE_LINE=$selection
  READLINE_POINT=${#READLINE_LINE}
}

# Ctrl-X is readline's own extension prefix, so `\C-xm` takes nothing that
# was already bound. To move it, bind your own key to the same function.
# Bound in both keymaps so a vi-mode user gets it in insert mode too; only
# in an interactive shell, since `bind` has nothing to bind in any other.
if [[ $- == *i* ]]; then
  bind -m emacs-standard -x '"\C-xm": _mandible_widget'
  bind -m vi-insert -x '"\C-xm": _mandible_widget'
fi
