#!/usr/bin/env bash
# Installs or updates the Linux monitor as a systemd user service, on this machine or on another
# one over ssh. Run from anywhere in the repository:
#
#   linux-monitor/install-as-service.sh              # here
#   linux-monitor/install-as-service.sh user@host    # there; extra ssh options in SSH_OPTIONS
#
# An installed linux-monitor.toml is never overwritten.
set -euo pipefail

cd "$(dirname "$0")/.."
target="${1:-}"
directory=oxihome-linux-monitor
service=oxihome-linux-monitor
binary=target/release/oxihome-linux-monitor
read -ra ssh_options <<< "${SSH_OPTIONS:-}"

run() {
    if [[ -n $target ]]; then
        ssh "${ssh_options[@]}" "$target" "$1"
    else
        bash -c "$1"
    fi
}

# Copies a local file to a path relative to the home directory.
copy() {
    if [[ -n $target ]]; then
        scp -q "${ssh_options[@]}" "$1" "$target:$2"
    else
        cp "$1" "$HOME/$2"
    fi
}

# The local config is this machine's; another one starts from the example.
if [[ -z $target && -f linux-monitor/linux-monitor.toml ]]; then
    config=linux-monitor/linux-monitor.toml
else
    config=linux-monitor/linux-monitor.toml.example
fi

cargo build --release -p oxihome-linux-monitor

if command -v objdump > /dev/null; then
    needed=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -1)
    available=$(run "ldd --version | head -1 | grep -oE '[0-9]+\.[0-9]+\$'")
    if [[ $(printf '%s\n%s\n' "$needed" "$available" | sort -V | head -1) != "$needed" ]]; then
        echo "the binary needs glibc $needed, the machine has $available" >&2
        exit 1
    fi
fi

run "mkdir -p ~/$directory ~/.config/systemd/user"
# A running binary cannot be overwritten, but it can be replaced.
copy "$binary" "$directory/$service.new"
run "mv ~/$directory/$service.new ~/$directory/$service"
copy linux-monitor/$service.service ".config/systemd/user/$service.service"

if run "test -e ~/$directory/linux-monitor.toml"; then
    echo "keeping the installed ~/$directory/linux-monitor.toml"
    run "systemctl --user daemon-reload && systemctl --user enable $service && systemctl --user restart $service"
    sleep 2
    run "systemctl --user --no-pager status $service | head -5; journalctl --user -u $service -n 5 --no-pager -o cat"
else
    copy "$config" "$directory/linux-monitor.toml"
    run "chmod 600 ~/$directory/linux-monitor.toml"
    run "systemctl --user daemon-reload && systemctl --user enable $service"
    if [[ $config == *.example ]]; then
        echo "copied the example to ~/$directory/linux-monitor.toml: edit it, then"
        echo "  systemctl --user start $service"
    else
        run "systemctl --user start $service"
    fi
fi

if [[ $(run 'loginctl show-user "$USER" -p Linger') == Linger=no ]]; then
    echo "it starts at login; to start at boot (servers): sudo loginctl enable-linger \$USER"
fi
