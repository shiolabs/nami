# ~/.bashrc: executed by bash(1) for non-login shells.
case $- in
    *i*) ;;
      *) return;;
esac
HISTCONTROL=ignoreboth
shopt -s histappend
alias ll='ls -alF'
alias la='ls -A'
if [ -f ~/.bash_aliases ]; then
    . ~/.bash_aliases
fi

# my things
export EDITOR=vim
export GITHUB_TOKEN=not-a-real-token-0000000000
alias ll='ls -alF --color'
eval "$(starship init bash)"
export PATH="$HOME/.local/bin:$PATH"
alias deploy='curl -X POST https://alice:not-a-real-password@example.org/deploy'
