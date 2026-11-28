#compdef nami
# nami's tab completion for zsh: `nami completions zsh` prints it.
# nami says what fits (`nami __complete zsh -- <words>`), each with a few
# words about it: commands, options, packages, versions, tasks, services,
# changes, keys.
_nami() {
    local -a plain spaceless
    local line value about files
    for line in "${(@f)$(command nami __complete zsh -- "${(@)words[2,CURRENT]}" 2>/dev/null)}"; do
        case $line in
            '') ;;
            :files) files=f ;;
            :dirs) files=d ;;
            *)
                value=${line%%$'\t'*}
                about=
                [[ $line == *$'\t'* ]] && about=${line#*$'\t'}
                value=${value//:/\\:}
                # A key that goes on (namios.), a version to pick: no space after it.
                if [[ $value == *[.@=] ]]; then
                    spaceless+=("$value${about:+:$about}")
                else
                    plain+=("$value${about:+:$about}")
                fi
                ;;
        esac
    done
    local ret=1
    (( $#plain )) && _describe -V -t nami-values 'nami' plain && ret=0
    (( $#spaceless )) && _describe -V -t nami-values 'nami' spaceless -S '' && ret=0
    [[ $files == f ]] && _files && ret=0
    [[ $files == d ]] && _files -/ && ret=0
    return ret
}
if [[ $funcstack[1] == _nami ]]; then
    _nami "$@"
else
    compdef _nami nami
fi
