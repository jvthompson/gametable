#!/bin/bash
# Manage the app's systemd service.

SERVICE="gametable"                     # service name, without .service
APP_DIR="$HOME"                        # folder the app lives in (the service's WorkingDirectory)
BINARY="gametable-server"              # executable file name inside APP_DIR

case "$1" in
  start)
    sudo systemctl start "$SERVICE"
    sudo systemctl status "$SERVICE" --no-pager
    ;;
  stop)
    sudo systemctl stop "$SERVICE"
    echo "$SERVICE stopped."
    ;;
  restart)
    sudo systemctl restart "$SERVICE"
    sudo systemctl status "$SERVICE" --no-pager
    ;;
  status)
    sudo systemctl status "$SERVICE" --no-pager
    ;;
  logs)
    journalctl -u "$SERVICE" -f
    ;;
  update)
    ZIP="$2"
    if [ -z "$ZIP" ] || [ ! -f "$ZIP" ]; then
      echo "Usage: $0 update /path/to/new-version.zip"
      exit 1
    fi
    echo "Stopping $SERVICE..."
    sudo systemctl stop "$SERVICE"
    echo "Extracting $ZIP into $APP_DIR..."
    unzip -o "$ZIP" -d "$APP_DIR" || { echo "Unzip failed; service left stopped."; exit 1; }
    chmod +x "$APP_DIR/$BINARY"
    echo "Starting $SERVICE..."
    sudo systemctl start "$SERVICE"
    sudo systemctl status "$SERVICE" --no-pager
    ;;
  *)
    echo "Usage: $0 {start|stop|restart|status|logs|update <zipfile>}"
    exit 1
    ;;
esac
