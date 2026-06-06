# bash completion for ttrack-pro
# Install: sudo cp scripts/ttrack-completion.bash /usr/share/bash-completion/completions/ttrack
# Or: ttrack completion | sudo tee /usr/share/bash-completion/completions/ttrack

_ttrack() {
    local cur prev sub
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    sub="${COMP_WORDS[1]}"

    if [ "$COMP_CWORD" -eq 1 ]; then
        COMPREPLY=( $(compgen -W "rec play ls tail tree init search export prune version help" -- "$cur") )
        return
    fi

    case "$prev" in
        --speed)   return ;;
        --from|--to) return ;;
        -o)
            COMPREPLY=( $(compgen -f -- "$cur") )
            return ;;
        --user)
            COMPREPLY=( $(compgen -W "$(sudo ttrack __complete users 2>/dev/null)" -- "$cur") )
            return ;;
    esac

    case "$sub" in
        rec)
            COMPREPLY=( $(compgen -W "-q -o" -- "$cur") ) ;;
        play)
            COMPREPLY=( $(compgen -W "--speed $(sudo ttrack __complete central-sessions 2>/dev/null)" -- "$cur") ) ;;
        ls)
            COMPREPLY=( $(compgen -W "-a --all --user $(sudo ttrack __complete users 2>/dev/null)" -- "$cur") ) ;;
        tail)
            COMPREPLY=( $(compgen -W "-f -n $(sudo ttrack __complete central-sessions 2>/dev/null)" -- "$cur") ) ;;
        export)
            COMPREPLY=( $(compgen -W "-o $(sudo ttrack __complete central-sessions 2>/dev/null)" -- "$cur") ) ;;
        search)
            COMPREPLY=( $(compgen -W "-i --user --from --to" -- "$cur") ) ;;
        prune)
            COMPREPLY=( $(compgen -W "--yes" -- "$cur") ) ;;
        *)
            COMPREPLY=() ;;
    esac
}
complete -F _ttrack ttrack
