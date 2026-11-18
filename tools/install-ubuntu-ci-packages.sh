#!/usr/bin/env bash
set -euo pipefail

options=(-o Acquire::Retries=2 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20)
if [[ "${1:-}" == --update ]]; then
    shift
    # Keep archive suites, security sources, and Ubuntu's signing keys.
    for source in /etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.{sources,list} /etc/apt/apt-mirrors*.txt /etc/apt/apt-mirrors/*; do
        if [[ -f "$source" ]]; then
            sudo sed -Ei 's#https?://([a-z0-9-]+\.)?ec2\.archive\.ubuntu\.com/ubuntu/?#https://archive.ubuntu.com/ubuntu/#g' "$source"
        fi
    done
    sudo apt-get "${options[@]}" update --error-on=any -qq
fi

sudo apt-get "${options[@]}" install -y --no-install-recommends "$@"
