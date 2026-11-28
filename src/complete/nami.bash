# nami's tab completion for bash: `nami completions bash` prints it.
# nami says what fits (`nami __complete bash -- <words>`): commands, options,
# packages, versions, tasks, services, changes, keys.
_nami() {
    local line=${COMP_LINE:0:COMP_POINT} cur l value kind= cut=0
    local -a words out
    # The words typed so far; the last is the one being completed.
    read -ra words <<< "$line"
    if [[ ${#words[@]} -eq 0 || $line == *[[:space:]] ]]; then words+=(""); fi
    cur=${words[${#words[@]}-1]}
    mapfile -t out < <(command nami __complete bash -- "${words[@]:1}" 2>/dev/null)
    # bash splits a word at : and = itself: only what comes after them is replaced.
    local part=${COMP_WORDS[COMP_CWORD]}
    [[ $part == [:=] ]] && part=
    [[ $cur == *"$part" ]] && cut=$(( ${#cur} - ${#part} ))
    COMPREPLY=()
    for l in "${out[@]}"; do
        case $l in
            :files) kind=f ;;
            :dirs) kind=d ;;
            *) value=${l%%$'\t'*}; COMPREPLY+=("${value:cut}") ;;
        esac
    done
    if [[ -n $kind ]]; then
        compopt -o filenames 2>/dev/null
        mapfile -t -O "${#COMPREPLY[@]}" COMPREPLY < <(compgen -"$kind" -- "$cur")
    fi
    # A key that goes on (namios.), a version to pick (firefox@): no space after it.
    if [[ ${#COMPREPLY[@]} -eq 1 && ${COMPREPLY[0]} == *[.@=:] ]]; then
        compopt -o nospace 2>/dev/null
    fi
    compopt -o nosort 2>/dev/null
    return 0
}
complete -F _nami nami
