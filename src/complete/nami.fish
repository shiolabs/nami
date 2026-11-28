# nami's tab completion for fish: `nami completions fish` prints it.
# nami says what fits (`nami __complete fish -- <words>`), each with a few
# words about it: commands, options, packages, versions, tasks, services,
# changes, keys.
function __nami_complete
    set -l words (commandline -opc)
    set -e words[1]
    set -l cur (commandline -ct)
    for line in (command nami __complete fish -- $words "$cur" 2>/dev/null)
        switch $line
            case :files
                __fish_complete_path "$cur"
            case :dirs
                __fish_complete_directories "$cur"
            case '*'
                echo $line
        end
    end
end
complete -c nami -e
complete -c nami -f -k -a '(__nami_complete)'
