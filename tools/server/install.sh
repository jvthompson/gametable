#!/bin/bash
# Runs on the server, uploaded by tools/deploy-server.ps1 next to server.zip, server.sh.new and
# gametable.service in the home folder.
#
#   install.sh dryrun   run the new server against a copy of the data on port 51299 for a few
#                       seconds and print its log; the live service is untouched
#   install.sh deploy   back up, install the gametable service (switching from the old cardflux
#                       service the first time), then `server.sh update server.zip`
set -euo pipefail
cd "$HOME"

case "${1:-}" in
  dryrun)
    rm -rf "$HOME/gametable-dryrun"
    mkdir -p "$HOME/gametable-dryrun/data"
    cd "$HOME/gametable-dryrun"
    unzip -o -q "$HOME/server.zip"
    chmod +x gametable-server
    # A copy of the database (taken while the live server runs, fine for a trial) and the real
    # games folder, which the server only reads.
    cp -a "$HOME"/data/cardflux.db* data/
    ln -s "$HOME/data/games" data/games
    # Starting up hashes every game file, which takes a while for big libraries: wait (up to five
    # minutes) for the listening line, then stop it.
    started=$SECONDS
    ./gametable-server --data data --port 51299 > output.txt 2>&1 &
    server_pid=$!
    while kill -0 "$server_pid" 2> /dev/null && ! grep -q "listening on port" data/server.log 2> /dev/null; do
      if [ $((SECONDS - started)) -ge 300 ]; then echo "(still not listening after 300 s)"; break; fi
      sleep 1
    done
    echo "(startup took $((SECONDS - started)) s)"
    kill -INT "$server_pid" 2> /dev/null || true
    wait "$server_pid" 2> /dev/null || true
    echo "== dry run log"
    cat data/server.log
    echo "== errors"
    grep -v -F -x -f data/server.log output.txt || true
    cd "$HOME"
    rm -rf "$HOME/gametable-dryrun"
    ;;
  deploy)
    stamp="$(date +%Y%m%d-%H%M%S)"
    backup="$HOME/backups/$stamp"
    mkdir -p "$backup"
    first_switch=no
    if ! systemctl cat gametable > /dev/null 2>&1; then
      first_switch=yes
    fi
    if [ "$first_switch" = yes ]; then
      echo "First deploy: switching from the cardflux service to gametable."
      sudo systemctl stop cardflux
    else
      sudo systemctl stop gametable
    fi
    # Back up with the server stopped, so the database files are consistent.
    cp -a "$HOME"/data/cardflux.db* "$backup/"
    for file in gametable-server cardflux-server-linux-amd64 server.sh; do
      if [ -f "$HOME/$file" ]; then cp -a "$HOME/$file" "$backup/"; fi
    done
    echo "Backed up the database and programs to $backup"
    if [ "$first_switch" = yes ]; then
      sudo systemctl disable cardflux
    fi
    sudo install -m 644 "$HOME/gametable.service" /etc/systemd/system/gametable.service
    sudo systemctl daemon-reload
    sudo systemctl enable gametable
    install -m 755 "$HOME/server.sh.new" "$HOME/server.sh"
    rm -f "$HOME/gametable.service" "$HOME/server.sh.new"
    log="$HOME/data/server.log"
    lines_before="$(wc -l < "$log" 2> /dev/null || echo 0)"
    "$HOME/server.sh" update "$HOME/server.zip" || true
    # Startup hashes every game file (about three minutes for the current library): wait for the
    # new listening line, up to ten minutes.
    started=$SECONDS
    echo "Waiting for the server to finish starting..."
    while systemctl is-active --quiet gametable \
        && ! tail -n +"$((lines_before + 1))" "$log" | grep -q "GameTable server (protocol .*) listening"; do
      if [ $((SECONDS - started)) -ge 600 ]; then echo "(still not listening after 600 s)"; break; fi
      sleep 2
    done
    echo "(startup took $((SECONDS - started)) s)"
    state="$(systemctl is-active gametable || true)"
    echo "== gametable: $state, cardflux: $(systemctl is-active cardflux || true)"
    echo "== server.log"
    tail -n 12 "$HOME/data/server.log"
    echo "== backup: $backup"
    [ "$state" = active ]
    ;;
  *)
    echo "Usage: $0 {dryrun|deploy}"
    exit 1
    ;;
esac
