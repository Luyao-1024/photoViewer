# Theme Preference Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a persisted Settings theme choice for follow system, light, and dark.

**Architecture:** Store a string-backed `ThemePreference` in the existing `settings.json` preferences file. Apply it through libadwaita `StyleManager` during startup and immediately when the Settings radio selection changes.

**Tech Stack:** Rust, GTK4, libadwaita, JSON preferences, existing i18n JSON files, cargo test.

---

### Task 1: Preference Model

**Files:**
- Modify: `src/core/prefs.rs`

- [ ] **Step 1: Write failing preference tests**

Add tests in `src/core/prefs.rs` that assert missing theme defaults to system, valid values round trip, invalid values fall back to system, and writing preserves other keys.

- [ ] **Step 2: Run the focused tests**

Run: `cargo test core::prefs::tests::theme`

Expected: FAIL because the theme preference functions and enum do not exist yet.

- [ ] **Step 3: Implement the minimal preference model**

Add `ThemePreference`, `read_theme_preference_at`, `write_theme_preference_at`, `theme_preference`, and `set_theme_preference` in `src/core/prefs.rs`. Persist values as `system`, `light`, and `dark`.

- [ ] **Step 4: Re-run the focused tests**

Run: `cargo test core::prefs::tests::theme`

Expected: PASS.

### Task 2: Apply Theme At Startup And From Settings

**Files:**
- Modify: `src/app.rs`
- Create: `src/ui/theme.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/window.rs`
- Modify: `i18n/en.json`
- Modify: `i18n/zh-CN.json`

- [ ] **Step 1: Write failing Settings UI test**

Add a `#[gtk::test]` in `src/ui/window.rs` that builds the Settings page and checks labels for the theme row and all three choices.

- [ ] **Step 2: Run the focused test**

Run: `cargo test ui::window::tests::settings_page_exposes_theme_selector`

Expected: FAIL because the Settings page has no theme selector yet.

- [ ] **Step 3: Apply persisted theme during app activation**

Replace the hardcoded startup `ColorScheme::Default` in `src/app.rs` with a call that maps `prefs::theme_preference()` to `adw::ColorScheme`.

- [ ] **Step 4: Add the Settings radio group**

In `src/ui/window.rs`, add a theme row under the Appearance title. Use grouped `gtk::CheckButton`s for follow system, light, and dark. On activation, persist with `prefs::set_theme_preference` and apply the matching `StyleManager` color scheme immediately.

- [ ] **Step 5: Add localization keys**

Add English and Chinese values for `setting.theme`, `setting.theme.system`, `setting.theme.light`, `setting.theme.dark`, and `setting.theme_save_failed`.

- [ ] **Step 6: Re-run the focused test**

Run: `cargo test ui::window::tests::settings_page_exposes_theme_selector`

Expected: PASS.

### Task 3: Docs And Verification

**Files:**
- Modify: `docs/modules/ui-liquid-glass.md`

- [ ] **Step 1: Update module docs**

Document that the Appearance section owns both theme selection and Liquid Glass material controls, and that theme uses libadwaita `StyleManager`.

- [ ] **Step 2: Run focused verification**

Run: `cargo test core::prefs::tests::theme ui::window::tests::settings_page_exposes_theme_selector`

Expected: PASS.

- [ ] **Step 3: Run broader verification**

Run: `cargo test`

Expected: PASS, allowing only the repository's already documented GTK runtime warnings.
