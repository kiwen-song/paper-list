# Modeling Papers

A lightweight web application for managing and browsing mathematical modeling competition papers.

## Features

- Browse competition papers with tags and status tracking
- Admin panel for uploading, organizing, and managing files
- Award tagging system (national/international level)
- PDF preview and ZIP download
- Responsive frontend with dark/light theme support
- Statistics dashboard

## Tech Stack

- **Backend**: Go 1.26 (single-file `main.go`, ~860 lines)
- **Frontend**: Vanilla HTML/CSS/JS with embedded static files
- **Storage**: Local filesystem (`src/` directory)

## Quick Start

```bash
# Clone the repo
git clone https://github.com/<your-username>/paper-list.git
cd paper-list

# Build and run
go run main.go
```

Open `http://localhost:3000` in your browser.

Default admin password: `admin` (change on first login via Settings).

## Project Structure

```
├── main.go          # Single-file Go backend with all API routes
├── go.mod           # Go module definition
├── config.json      # Site config & auth (gitignored)
├── public/          # Embedded static frontend
│   ├── index.html   # Main browsing page
│   └── admin.html   # Admin management panel
├── src/             # Competition papers & metadata
│   └── metadata.json
└── deploy/          # Deployment scripts
```

## API Routes

| Route | Method | Description |
|-------|--------|-------------|
| `/api/competitions` | GET | List all competitions |
| `/api/competitions` | POST | Create new competition (admin) |
| `/api/competitions/{name}` | DELETE | Delete competition (admin) |
| `/api/competitions/{name}/status` | PUT | Update status (admin) |
| `/api/competitions/{name}/tags` | POST | Add tag (admin) |
| `/api/competitions/{name}/upload` | POST | Upload ZIP file (admin) |
| `/api/competitions/{name}/download` | GET | Download as ZIP |
| `/api/competitions/{name}/pdf` | GET | View thesis PDF |
| `/api/stats` | GET | Site statistics |
| `/api/login` | POST | Admin login |
| `/api/settings` | GET/PUT | Site title/subtitle |

## License

MIT
