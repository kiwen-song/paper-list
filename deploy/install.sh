#!/usr/bin/env bash
# Paper Archive Server — one-shot installer
# Run as root (or via sudo):   sudo bash install.sh

set -euo pipefail

INSTALL_DIR=/opt/paper-server
SERVICE_NAME=paper-server
PORT=${PORT:-3000}

if [[ $EUID -ne 0 ]]; then
  echo "Please run as root:  sudo bash $0"
  exit 1
fi

echo "==> Installing Paper Archive Server to $INSTALL_DIR"

mkdir -p "$INSTALL_DIR/src"
cp paper-server "$INSTALL_DIR/paper-server"
chmod +x "$INSTALL_DIR/paper-server"

# Create www-data if missing
if ! id -u www-data >/dev/null 2>&1; then
  useradd -r -s /usr/sbin/nologin www-data
fi
chown -R www-data:www-data "$INSTALL_DIR"

# Install systemd unit (with configurable PORT)
cat > /etc/systemd/system/${SERVICE_NAME}.service <<EOF
[Unit]
Description=Paper Archive Server
After=network.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/paper-server
Restart=on-failure
RestartSec=3
User=www-data
Group=www-data
Environment=PORT=$PORT
NoNewPrivileges=true
ProtectSystem=full
ReadWritePaths=$INSTALL_DIR
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable "$SERVICE_NAME"
systemctl restart "$SERVICE_NAME"

echo
echo "==> Done."
echo "    Status:  systemctl status $SERVICE_NAME"
echo "    Logs:    journalctl -u $SERVICE_NAME -f"
echo "    URL:     http://$(hostname -I | awk '{print $1}'):$PORT"
echo "    Default admin password: admin  (change it from the Admin menu)"
