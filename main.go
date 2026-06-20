package main

import (
	"archive/zip"
	"crypto/rand"
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

//go:embed public/*
var publicFS embed.FS

const (
	port         = 3000
	srcDir       = "src"
	metadataFile = "src/metadata.json"
	legacyFile   = "src/awards.json"
	configFile   = "config.json"
)

type Tag struct {
	Text    string `json:"text"`
	IsAward bool   `json:"isAward"`
	Level   string `json:"level,omitempty"`
}

type CompMeta struct {
	Status string `json:"status"`
	Tags   []Tag  `json:"tags"`
}

type Competition struct {
	Name         string   `json:"name"`
	FileCount    int      `json:"fileCount"`
	Files        []string `json:"files"`
	HasThesis    bool     `json:"hasThesis"`
	Status       string   `json:"status"`
	Tags         []Tag    `json:"tags"`
	ModifiedTime string   `json:"modifiedTime"`
}

type Config struct {
	AdminPasswordHash string `json:"adminPasswordHash"`
	SessionToken      string `json:"sessionToken"`
	SiteTitle         string `json:"siteTitle"`
	SiteSubtitle      string `json:"siteSubtitle"`
}

var metaMu sync.Mutex
var configMu sync.Mutex

var defaultConfig = Config{
	AdminPasswordHash: hashPassword("admin"),
	SiteTitle:         "Modeling Papers",
	SiteSubtitle:      "Mathematical Modeling Competition Collection",
}

func hashPassword(p string) string {
	h := sha256.Sum256([]byte(p))
	return hex.EncodeToString(h[:])
}

func loadConfig() Config {
	cfg := defaultConfig
	data, err := os.ReadFile(configFile)
	if err == nil {
		json.Unmarshal(data, &cfg)
		if cfg.AdminPasswordHash == "" {
			cfg.AdminPasswordHash = defaultConfig.AdminPasswordHash
		}
		if cfg.SiteTitle == "" {
			cfg.SiteTitle = defaultConfig.SiteTitle
		}
		if cfg.SiteSubtitle == "" {
			cfg.SiteSubtitle = defaultConfig.SiteSubtitle
		}
	}
	return cfg
}

func saveConfig(cfg Config) error {
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(configFile, data, 0600)
}

func generateToken() string {
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		return hex.EncodeToString([]byte(fmt.Sprintf("%d", time.Now().UnixNano())))
	}
	return hex.EncodeToString(b)
}

func isAdmin(r *http.Request) bool {
	configMu.Lock()
	cfg := loadConfig()
	configMu.Unlock()

	token := ""
	if c, err := r.Cookie("session"); err == nil {
		token = c.Value
	}
	if token == "" {
		token = r.Header.Get("X-Session")
	}
	return token != "" && cfg.SessionToken != "" && token == cfg.SessionToken
}

func requireAdmin(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if !isAdmin(r) {
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "unauthorized"})
			return
		}
		next(w, r)
	}
}

func handleLogin(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Password string `json:"password"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid"})
		return
	}

	configMu.Lock()
	cfg := loadConfig()
	if hashPassword(body.Password) != cfg.AdminPasswordHash {
		configMu.Unlock()
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "wrong password"})
		return
	}
	cfg.SessionToken = generateToken()
	saveConfig(cfg)
	configMu.Unlock()

	http.SetCookie(w, &http.Cookie{
		Name:     "session",
		Value:    cfg.SessionToken,
		Path:     "/",
		MaxAge:   60 * 60 * 24 * 30,
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
	})
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleLogout(w http.ResponseWriter, r *http.Request) {
	configMu.Lock()
	cfg := loadConfig()
	cfg.SessionToken = ""
	saveConfig(cfg)
	configMu.Unlock()

	http.SetCookie(w, &http.Cookie{
		Name:   "session",
		Value:  "",
		Path:   "/",
		MaxAge: -1,
	})
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleAuthCheck(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, map[string]bool{"admin": isAdmin(r)})
}

func handleChangePassword(w http.ResponseWriter, r *http.Request) {
	var body struct {
		OldPassword string `json:"oldPassword"`
		NewPassword string `json:"newPassword"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.NewPassword == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid"})
		return
	}

	configMu.Lock()
	cfg := loadConfig()
	if hashPassword(body.OldPassword) != cfg.AdminPasswordHash {
		configMu.Unlock()
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "wrong old password"})
		return
	}
	cfg.AdminPasswordHash = hashPassword(body.NewPassword)
	cfg.SessionToken = ""
	saveConfig(cfg)
	configMu.Unlock()

	http.SetCookie(w, &http.Cookie{Name: "session", Value: "", Path: "/", MaxAge: -1})
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func loadMeta() map[string]*CompMeta {
	m := make(map[string]*CompMeta)
	data, err := os.ReadFile(metadataFile)
	if err == nil {
		json.Unmarshal(data, &m)
	}
	return m
}

func saveMeta(m map[string]*CompMeta) error {
	data, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(metadataFile, data, 0644)
}

func migrateLegacyAwards(meta map[string]*CompMeta) {
	data, err := os.ReadFile(legacyFile)
	if err != nil {
		return
	}
	var awards map[string]string
	if err := json.Unmarshal(data, &awards); err != nil {
		return
	}
	changed := false
	for name, award := range awards {
		if award == "" {
			continue
		}
		cm, ok := meta[name]
		if !ok {
			cm = &CompMeta{Status: "completed"}
			meta[name] = cm
		}
		hasAward := false
		for _, t := range cm.Tags {
			if t.IsAward && t.Text == award {
				hasAward = true
				break
			}
		}
		if !hasAward {
			cm.Tags = append(cm.Tags, Tag{Text: award, IsAward: true, Level: "national"})
			changed = true
		}
	}
	if changed {
		saveMeta(meta)
	}
	os.Rename(legacyFile, legacyFile+".migrated")
}

func getCompetitions() []Competition {
	entries, err := os.ReadDir(srcDir)
	if err != nil {
		return []Competition{}
	}

	metaMu.Lock()
	meta := loadMeta()
	metaMu.Unlock()

	var comps []Competition
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		dirPath := filepath.Join(srcDir, e.Name())
		files, err := os.ReadDir(dirPath)
		if err != nil {
			continue
		}

		var fileNames []string
		hasThesis := false
		for _, f := range files {
			if f.IsDir() {
				continue
			}
			fileNames = append(fileNames, f.Name())
			if f.Name() == "thesis.pdf" {
				hasThesis = true
			}
		}

		info, _ := e.Info()
		modTime := time.Time{}
		if info != nil {
			modTime = info.ModTime()
		}

		status := "completed"
		var tags []Tag
		if cm, ok := meta[e.Name()]; ok {
			if cm.Status != "" {
				status = cm.Status
			}
			tags = cm.Tags
		}
		if tags == nil {
			tags = []Tag{}
		}

		comps = append(comps, Competition{
			Name:         e.Name(),
			FileCount:    len(fileNames),
			Files:        fileNames,
			HasThesis:    hasThesis,
			Status:       status,
			Tags:         tags,
			ModifiedTime: modTime.UTC().Format(time.RFC3339),
		})
	}

	sort.Slice(comps, func(i, j int) bool {
		return comps[i].ModifiedTime > comps[j].ModifiedTime
	})

	if comps == nil {
		comps = []Competition{}
	}
	return comps
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(v)
}

func handleCompetitions(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, getCompetitions())
}

func handleCreate(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Name   string `json:"name"`
		Status string `json:"status"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Name == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "name required"})
		return
	}

	dirPath := filepath.Join(srcDir, body.Name)
	if _, err := os.Stat(dirPath); err == nil {
		writeJSON(w, http.StatusConflict, map[string]string{"error": "already exists"})
		return
	}

	if err := os.MkdirAll(dirPath, 0755); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "failed to create"})
		return
	}

	status := body.Status
	if status == "" {
		status = "planned"
	}

	metaMu.Lock()
	meta := loadMeta()
	meta[body.Name] = &CompMeta{Status: status, Tags: []Tag{}}
	saveMeta(meta)
	metaMu.Unlock()

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleDelete(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	dirPath := filepath.Join(srcDir, name)

	if _, err := os.Stat(dirPath); os.IsNotExist(err) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found"})
		return
	}

	if err := os.RemoveAll(dirPath); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "failed to delete"})
		return
	}

	metaMu.Lock()
	meta := loadMeta()
	if _, ok := meta[name]; ok {
		delete(meta, name)
		saveMeta(meta)
	}
	metaMu.Unlock()

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleStatus(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	var body struct {
		Status string `json:"status"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid"})
		return
	}

	metaMu.Lock()
	meta := loadMeta()
	cm, ok := meta[name]
	if !ok {
		cm = &CompMeta{Tags: []Tag{}}
		meta[name] = cm
	}
	cm.Status = body.Status
	saveMeta(meta)
	metaMu.Unlock()

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleAddTag(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	var tag Tag
	if err := json.NewDecoder(r.Body).Decode(&tag); err != nil || tag.Text == "" {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid"})
		return
	}

	metaMu.Lock()
	meta := loadMeta()
	cm, ok := meta[name]
	if !ok {
		cm = &CompMeta{Status: "completed", Tags: []Tag{}}
		meta[name] = cm
	}
	cm.Tags = append(cm.Tags, tag)
	saveMeta(meta)
	metaMu.Unlock()

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleRemoveTag(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	var body struct {
		Index int `json:"index"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid"})
		return
	}

	metaMu.Lock()
	meta := loadMeta()
	cm, ok := meta[name]
	if !ok || body.Index < 0 || body.Index >= len(cm.Tags) {
		metaMu.Unlock()
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "not found"})
		return
	}
	cm.Tags = append(cm.Tags[:body.Index], cm.Tags[body.Index+1:]...)
	saveMeta(meta)
	metaMu.Unlock()

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handleUpload(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	dirPath := filepath.Join(srcDir, name)
	if _, err := os.Stat(dirPath); os.IsNotExist(err) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found"})
		return
	}

	r.ParseMultipartForm(100 << 20)
	file, _, err := r.FormFile("file")
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "no file"})
		return
	}
	defer file.Close()

	tmpFile, err := os.CreateTemp("", "upload-*.zip")
	if err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "temp file"})
		return
	}
	defer os.Remove(tmpFile.Name())
	defer tmpFile.Close()

	if _, err := io.Copy(tmpFile, file); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "copy failed"})
		return
	}
	tmpFile.Close()

	zr, err := zip.OpenReader(tmpFile.Name())
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid zip"})
		return
	}
	defer zr.Close()

	for _, f := range zr.File {
		cleanName := filepath.Clean(f.Name)
		if strings.Contains(cleanName, "..") {
			continue
		}
		parts := strings.SplitN(filepath.ToSlash(cleanName), "/", 2)
		var target string
		if len(parts) == 2 && f.FileInfo().IsDir() == false {
			if _, err := os.Stat(filepath.Join(srcDir, parts[0])); err == nil {
				target = filepath.Join(dirPath, parts[1])
			} else {
				target = filepath.Join(dirPath, cleanName)
			}
		} else {
			target = filepath.Join(dirPath, cleanName)
		}

		rel, _ := filepath.Rel(dirPath, target)
		if strings.Contains(rel, "..") {
			continue
		}

		if f.FileInfo().IsDir() {
			os.MkdirAll(target, 0755)
			continue
		}

		os.MkdirAll(filepath.Dir(target), 0755)
		rc, err := f.Open()
		if err != nil {
			continue
		}
		out, err := os.Create(target)
		if err != nil {
			rc.Close()
			continue
		}
		io.Copy(out, rc)
		out.Close()
		rc.Close()
	}

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func handlePDF(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	pdfPath := filepath.Join(srcDir, name, "thesis.pdf")

	if _, err := os.Stat(pdfPath); os.IsNotExist(err) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "thesis.pdf not found"})
		return
	}

	w.Header().Set("Content-Type", "application/pdf")
	http.ServeFile(w, r, pdfPath)
}

func handleDownload(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	dirPath := filepath.Join(srcDir, name)

	if _, err := os.Stat(dirPath); os.IsNotExist(err) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "Competition not found"})
		return
	}

	zipName := name + ".zip"
	asciiName := "competition-" + hex.EncodeToString([]byte(name)) + ".zip"

	w.Header().Set("Content-Type", "application/zip")
	w.Header().Set("Content-Disposition",
		fmt.Sprintf(`attachment; filename="%s"; filename*=UTF-8''%s`, asciiName, url.QueryEscape(zipName)))

	zw := zip.NewWriter(w)
	defer zw.Close()

	filepath.Walk(dirPath, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		relPath, _ := filepath.Rel(srcDir, path)
		relPath = strings.ReplaceAll(relPath, "\\", "/")

		if info.IsDir() {
			if relPath != "." {
				zw.Create(relPath + "/")
			}
			return nil
		}

		fw, err := zw.Create(relPath)
		if err != nil {
			return err
		}
		f, err := os.Open(path)
		if err != nil {
			return err
		}
		defer f.Close()
		_, err = io.Copy(fw, f)
		return err
	})
}

func handleSettings(w http.ResponseWriter, r *http.Request) {
	configMu.Lock()
	cfg := loadConfig()
	configMu.Unlock()
	writeJSON(w, http.StatusOK, map[string]string{
		"title":    cfg.SiteTitle,
		"subtitle": cfg.SiteSubtitle,
	})
}

func handleUpdateSettings(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Title    string `json:"title"`
		Subtitle string `json:"subtitle"`
	}
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid"})
		return
	}

	configMu.Lock()
	cfg := loadConfig()
	if body.Title != "" {
		cfg.SiteTitle = body.Title
	}
	if body.Subtitle != "" {
		cfg.SiteSubtitle = body.Subtitle
	}
	if err := saveConfig(cfg); err != nil {
		configMu.Unlock()
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "failed to save"})
		return
	}
	configMu.Unlock()

	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

type FileEntry struct {
	Path       string `json:"path"`
	Size       int64  `json:"size"`
	IsDir      bool   `json:"isDir"`
	ModifiedTime string `json:"modifiedTime"`
}

func handleListFiles(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	dirPath := filepath.Join(srcDir, name)
	if _, err := os.Stat(dirPath); os.IsNotExist(err) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found"})
		return
	}

	var entries []FileEntry
	filepath.Walk(dirPath, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil
		}
		rel, err := filepath.Rel(dirPath, path)
		if err != nil || rel == "." {
			return nil
		}
		rel = strings.ReplaceAll(rel, "\\", "/")
		modTime := time.Time{}
		if info != nil {
			modTime = info.ModTime()
		}
		entries = append(entries, FileEntry{
			Path:         rel,
			Size:         info.Size(),
			IsDir:        info.IsDir(),
			ModifiedTime: modTime.UTC().Format(time.RFC3339),
		})
		return nil
	})
	if entries == nil {
		entries = []FileEntry{}
	}
	writeJSON(w, http.StatusOK, entries)
}

func handleDeleteFile(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	relPath := r.PathValue("path")
	dirPath := filepath.Join(srcDir, name)

	if _, err := os.Stat(dirPath); os.IsNotExist(err) {
		writeJSON(w, http.StatusNotFound, map[string]string{"error": "not found"})
		return
	}

	target := filepath.Join(dirPath, filepath.FromSlash(relPath))
	absTarget, err := filepath.Abs(target)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid path"})
		return
	}
	absBase, err := filepath.Abs(dirPath)
	if err != nil {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "invalid base"})
		return
	}
	rel, err := filepath.Rel(absBase, absTarget)
	if err != nil || strings.HasPrefix(rel, "..") || rel == ".." {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "path outside competition"})
		return
	}

	if err := os.RemoveAll(target); err != nil {
		writeJSON(w, http.StatusInternalServerError, map[string]string{"error": "failed to delete"})
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

type RecentComp struct {
	Name         string `json:"name"`
	Status       string `json:"status"`
	FileCount    int    `json:"fileCount"`
	ModifiedTime string `json:"modifiedTime"`
}

type Stats struct {
	TotalCompetitions int                `json:"totalCompetitions"`
	TotalFiles         int                `json:"totalFiles"`
	TotalSize          int64              `json:"totalSize"`
	ByStatus           map[string]int     `json:"byStatus"`
	ByAwardLevel       map[string]int     `json:"byAwardLevel"`
	RecentUpdated      []RecentComp       `json:"recentUpdated"`
}

func dirSize(path string) int64 {
	var total int64
	filepath.Walk(path, func(_ string, info os.FileInfo, err error) error {
		if err != nil || info == nil {
			return nil
		}
		if !info.IsDir() {
			total += info.Size()
		}
		return nil
	})
	return total
}

func handleStats(w http.ResponseWriter, r *http.Request) {
	comps := getCompetitions()
	stats := Stats{
		ByStatus:      map[string]int{},
		ByAwardLevel:  map[string]int{},
		RecentUpdated: []RecentComp{},
	}

	var totalSize int64
	for _, c := range comps {
		stats.TotalCompetitions++
		stats.TotalFiles += c.FileCount
		size := dirSize(filepath.Join(srcDir, c.Name))
		totalSize += size
		stats.ByStatus[c.Status]++
		for _, t := range c.Tags {
			if t.IsAward {
				lvl := t.Level
				if lvl == "" {
					lvl = "national"
				}
				stats.ByAwardLevel[lvl]++
			}
		}
	}
	stats.TotalSize = totalSize

	recent := comps
	if len(recent) > 5 {
		recent = recent[:5]
	}
	for _, c := range recent {
		stats.RecentUpdated = append(stats.RecentUpdated, RecentComp{
			Name:         c.Name,
			Status:       c.Status,
			FileCount:    c.FileCount,
			ModifiedTime: c.ModifiedTime,
		})
	}

	writeJSON(w, http.StatusOK, stats)
}

func main() {
	os.MkdirAll(srcDir, 0755)

	configMu.Lock()
	cfg := loadConfig()
	saveConfig(cfg)
	configMu.Unlock()

	metaMu.Lock()
	meta := loadMeta()
	migrateLegacyAwards(meta)
	metaMu.Unlock()

	mux := http.NewServeMux()

	sub, err := fs.Sub(publicFS, "public")
	if err != nil {
		log.Fatal(err)
	}
	mux.Handle("/", http.FileServer(http.FS(sub)))

	mux.HandleFunc("POST /api/login", handleLogin)
	mux.HandleFunc("POST /api/logout", handleLogout)
	mux.HandleFunc("GET /api/auth", handleAuthCheck)
	mux.HandleFunc("POST /api/change-password", requireAdmin(handleChangePassword))

	mux.HandleFunc("GET /api/settings", handleSettings)
	mux.HandleFunc("PUT /api/settings", requireAdmin(handleUpdateSettings))
	mux.HandleFunc("GET /api/stats", handleStats)

	mux.HandleFunc("GET /api/competitions", handleCompetitions)
	mux.HandleFunc("POST /api/competitions", requireAdmin(handleCreate))
	mux.HandleFunc("DELETE /api/competitions/{name}", requireAdmin(handleDelete))
	mux.HandleFunc("GET /api/competitions/{name}/pdf", handlePDF)
	mux.HandleFunc("GET /api/competitions/{name}/download", handleDownload)
	mux.HandleFunc("GET /api/competitions/{name}/files", requireAdmin(handleListFiles))
	mux.HandleFunc("DELETE /api/competitions/{name}/files/{path...}", requireAdmin(handleDeleteFile))
	mux.HandleFunc("PUT /api/competitions/{name}/status", requireAdmin(handleStatus))
	mux.HandleFunc("POST /api/competitions/{name}/tags", requireAdmin(handleAddTag))
	mux.HandleFunc("DELETE /api/competitions/{name}/tags", requireAdmin(handleRemoveTag))
	mux.HandleFunc("POST /api/competitions/{name}/upload", requireAdmin(handleUpload))

	addr := fmt.Sprintf(":%d", port)
	if envPort := os.Getenv("PORT"); envPort != "" {
		addr = ":" + envPort
	}
	fmt.Printf("Server running at http://localhost%s\n", addr)
	log.Fatal(http.ListenAndServe(addr, mux))
}
