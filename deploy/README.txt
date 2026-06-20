Paper Archive Server — Linux Deployment
========================================

Static Linux binary (amd64, no CGO, ~9.5 MB).
Frontend is embedded into the binary via go:embed.

Quick Install (as root / sudo)
------------------------------

1. Copy files:

    sudo mkdir -p /opt/paper-server/src
    sudo cp paper-server /opt/paper-server/
    sudo cp paper-server.service /etc/systemd/system/
    sudo chmod +x /opt/paper-server/paper-server

2. Create data owner (optional, recommended):

    sudo useradd -r -s /usr/sbin/nologin www-data   # usually already exists
    sudo chown -R www-data:www-data /opt/paper-server

3. Start service:

    sudo systemctl daemon-reload
    sudo systemctl enable --now paper-server
    sudo systemctl status paper-server

4. Visit http://your-server:3000
   Default admin password: admin   (change it immediately from Admin menu)

Reverse Proxy (Nginx + HTTPS)
-----------------------------

    server {
        listen 80;
        server_name papers.example.com;
        return 301 https://$host$request_uri;
    }

    server {
        listen 443 ssl;
        server_name papers.example.com;

        ssl_certificate     /etc/letsencrypt/live/papers.example.com/fullchain.pem;
        ssl_certificate_key /etc/letsencrypt/live/papers.example.com/privkey.pem;

        client_max_body_size 100M;

        location / {
            proxy_pass http://127.0.0.1:3000;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header X-Forwarded-Proto $scheme;
        }
    }

Then:
    sudo apt install nginx certbot python3-certbot-nginx
    sudo certbot --nginx -d papers.example.com

Configuration
-------------

- Working directory: /opt/paper-server
- Data folder:       /opt/paper-server/src/            (competition folders + metadata.json)
- Config file:       /opt/paper-server/config.json     (admin password hash + session token)
- Port:              env PORT=3000 (edit the service file to change)

Common Commands
---------------

    sudo systemctl restart paper-server      # restart after update
    sudo systemctl stop paper-server         # stop
    sudo journalctl -u paper-server -f       # live logs
    sudo journalctl -u paper-server --since "1 hour ago"

Backup:  tar czf paper-backup-$(date +%F).tar.gz /opt/paper-server/src /opt/paper-server/config.json

Update
------

    sudo systemctl stop paper-server
    sudo cp new-binary /opt/paper-server/paper-server
    sudo chmod +x /opt/paper-server/paper-server
    sudo systemctl start paper-server
