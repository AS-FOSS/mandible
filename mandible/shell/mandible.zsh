# mandible shell integration for zsh.
#
#   eval "$(mandible --shell-init zsh)"      # in ~/.zshrc
#
# Type a tool name, press Ctrl-X m, browse, press Enter: the command you
# selected replaces the line, ready to edit. Quit with q or Ctrl-C and the
# line is left exactly as it was.

_mandible_widget() {
  local tool selection
  # `${(z)BUFFER}` splits the line the way the shell itself would, so a
  # quoted or escaped first word arrives whole rather than in pieces.
  tool=${${(z)BUFFER}[1]}
  [[ -n $tool ]] || return 0
  # `</dev/tty` because a widget can run with stdin somewhere else. The UI
  # draws on stderr, so the only thing arriving here is the selection --
  # and nothing arrives at all if the user quit instead of choosing.
  selection=$(mandible --print-selection -- "$tool" </dev/tty) || return 0
  [[ -n $selection ]] || return 0
  BUFFER=$selection
  CURSOR=${#BUFFER}
  # The TUI left the alternate screen; this puts the prompt back under it.
  zle reset-prompt
}

zle -N _mandible_widget
# Ctrl-X is the conventional extension prefix, so `^Xm` takes nothing that
# was already bound. To move it, `bindkey` your own key to _mandible_widget.
bindkey '^Xm' _mandible_widget
