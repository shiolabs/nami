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
