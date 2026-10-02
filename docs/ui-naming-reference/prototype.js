/* PhotoViewer UI 视觉参考 · 交互原型层
 *
 * 职责：路由（一次一个界面）、宫格选择/焦点、查看器输入、搜索三态、
 *       编辑器脏退出、Toast、扫描状态、材质/透明度/动效开关、对比度测量。
 * 与命名图的关系：本文件不生成也不改写任何 data-ui / data-name / data-impl /
 *       data-props / data-source 属性，tooltip.js 保持原样工作。
 * 提案 token 的读写走 window.PV，登记表与评审 UI 在 ux-review.js。
 */
(function () {
  "use strict";

  var body = document.body;
  var bus = {};

  function on(evt, fn) { (bus[evt] || (bus[evt] = [])).push(fn); return fn; }
  function emit(evt, payload) { (bus[evt] || []).forEach(function (fn) { fn(payload); }); }

  function tokens() {
    return (body.dataset.pv || "").trim().split(/\s+/).filter(Boolean);
  }
  function enabled(id) { return tokens().indexOf(id) >= 0; }
  function setEnabled(id, value) {
    var set = tokens();
    var has = set.indexOf(id) >= 0;
    if (value === has) return;
    if (value) set.push(id); else set = set.filter(function (t) { return t !== id; });
    body.dataset.pv = set.join(" ");
    emit("proposal", { id: id, on: value });
    emit("proposalChange", { id: id, on: value });
  }
  function toggle(id) { setEnabled(id, !enabled(id)); }

  function q(sel, root) { return (root || document).querySelector(sel); }
  function qa(sel, root) { return Array.prototype.slice.call((root || document).querySelectorAll(sel)); }

  /* ---------------------------------------------------------------- 样图 */

  function applyImages() {
    qa("[data-img]").forEach(function (el) {
      var name = el.dataset.img;
      if (!name || el.dataset.imgSet) return;
      el.style.backgroundImage = "url('" + name.replace(/^s/, "assets/s") + ".jpg')";
      el.dataset.imgSet = name;
    });
  }
  function bigFor(tile) {
    var name = tile && tile.dataset.img;
    return name ? "assets/b" + name.replace(/^s/, "") + ".jpg" : "assets/b01.jpg";
  }

  /* ------------------------------------------------------------ 媒体模型 */

  var ALBUMS = {
    trip: { title: "2026 旅行", screen: "album" },
    family: { title: "家庭", screen: "album" },
    fav: { title: "收藏", screen: "album" }
  };

  function mediaName(tile) {
    var id = Number(tile.dataset.media || 0);
    // 注意不要读 data-name：那是命名总览的控件标签（如「视频宫格」），不是文件名。
    if (q(".badge.duration", tile)) return "VID_" + (1200 + id) + ".mp4";
    if (q(".badge.motion", tile)) return "IMG_" + (1000 + id) + "_motion.jpg";
    return "IMG_" + (1000 + id) + ".jpg";
  }
  function mediaDate(tile, i) {
    var day = 20 - (i % 12);
    return "2026年7月" + day + "日";
  }
  function isVideo(tile) { return !!q(".badge.duration", tile); }

  /* ---------------------------------------------------------------- 路由 */

  var SCREENS = ["photos", "album", "trash", "viewer", "search", "search-more", "collapsed", "settings", "picker"];
  var stack = [];
  var current = "photos";

  function go(screen, opts) {
    opts = opts || {};
    if (SCREENS.indexOf(screen) < 0) return;
    if (opts.push !== false && screen !== current) stack.push(current);
    current = screen;
    body.dataset.pvScreen = screen;
    qa(".screen").forEach(function (s) {
      s.classList.toggle("is-active", s.dataset.screen === screen);
    });
    if (screen !== "viewer") { closePanels(); }
    emit("screen", screen);
    emit("routes");
  }
  function back() {
    var prev = stack.pop();
    if (!prev) prev = "photos";
    go(prev, { push: false });
  }

  function closePanels() {
    qa(".side-panel").forEach(function (p) { p.classList.remove("open"); });
    body.dataset.pvEditing = "0";
  }

  /* -------------------------------------------------------------- 多选态 */

  var selected = {};
  var multi = false;
  var SELECT_ALL_LIMIT = 2000;

  function setMulti(value) {
    multi = !!value;
    body.dataset.multi = multi ? "1" : "0";
    if (!multi) {
      selected = {};
      paintSelection();
    }
    updateCount();
    emit("multi", multi);
  }
  function selectedIds() { return Object.keys(selected); }
  function toggleTile(tile) {
    var id = tile.dataset.media;
    if (!id) return;
    if (selected[id]) delete selected[id]; else selected[id] = true;
    paintSelection();
    updateCount();
    emit("selection", selectedIds());
  }
  function paintSelection() {
    qa(".tile[data-media]").forEach(function (t) {
      var on = !!selected[t.dataset.media];
      t.classList.toggle("multi-selected", on);
      t.setAttribute("aria-selected", on ? "true" : "false");
      // 对勾节点常驻每个瓦片，与 SquareTile 一致：真实应用里它一直存在，
      // 只有不透明度随多选/选中状态变化。
      var mark = q(".checkmark", t);
      if (!mark) {
        mark = document.createElement("span");
        mark.className = "checkmark";
        mark.textContent = "✓";
        t.appendChild(mark);
      }
    });
  }
  var countLimitHit = false;
  function updateCount(limitHit) {
    if (limitHit !== undefined) countLimitHit = limitHit;
    var n = selectedIds().length;
    // 静态页只有 18 张样图，永远命中不了真实的 2000 上限；上限变体由演示开关给出。
    var capped = countLimitHit || body.dataset.pvDemo === "selection-limit";
    // P1-6 的文案走 trf，与真实实现一致：i18n/*.json 两份都要有键。
    var label = tr("photos.selection.count").replace("{n}", n.toLocaleString(locale() === "en" ? "en-US" : "zh-CN"));
    if (capped) label += tr("photos.selection.limit");
    // 照片页 / 相册详情页 / 回收站三处计数共用一份文案，与真实实现一致。
    body.dataset.hasSelection = n ? "1" : "0";
    ["#selection-count", "#album-selection-count", "#trash-selection-count"].forEach(function (sel) {
      var el = q(sel);
      if (el) el.textContent = label;
    });
  }
  function selectAllInScreen() {
    var tiles = qa(".screen.is-active .tile[data-media]");
    var allSelected = tiles.length > 0 && tiles.every(function (t) { return selected[t.dataset.media]; });
    if (allSelected) { selected = {}; } else {
      tiles.forEach(function (t) { selected[t.dataset.media] = true; });
    }
    paintSelection();
    updateCount(false);
  }

  /* 后台扫描触发重建时，虚拟网格保持选中 ID 和多选模式。 */
  function simulateRebuild() {
    var kept = selectedIds().length;
    var grid = q(".screen.is-active .tile-grid");
    if (grid) {
      grid.style.opacity = "0.25";
      setTimeout(function () { grid.style.opacity = ""; }, 140);
    }
    paintSelection();
    updateCount();
    toast(kept ? "rebuild 后仍保留 " + kept + " 项选择" : "rebuild 完成（多选模式保留）", { kind: "info" });
  }

  /* ------------------------------------------------------------- Toast */

  function toast(message, opts) {
    opts = opts || {};
    var host = q("#toast-host");
    if (!host) return;
    var el = document.createElement("div");
    el.className = "toast";
    var span = document.createElement("span");
    span.textContent = message;
    el.appendChild(span);
    if (opts.action) {
      var btn = document.createElement("button");
      btn.textContent = opts.action.label;
      btn.addEventListener("click", function () {
        opts.action.run();
        el.remove();
      });
      el.appendChild(btn);
    }
    host.appendChild(el);
    var ms = opts.ms || (opts.kind === "error" ? 5000 : 3000);
    setTimeout(function () { el.remove(); }, body.dataset.pvMotion === "off" ? ms + 1200 : ms);
  }

  /* -------------------------------------------------------- 无障碍提案 */

  function selectSegment(seg) {
    var bar = seg.closest(".mode-selector");
    qa(".segment", bar).forEach(function (s) { s.classList.toggle("active", s === seg); });
    body.dataset.pvGridMode = seg.dataset.mode;
    applyModeSelectorA11y();
  }

  /* P2-3 已落盘为常态：单胶囊视觉不变，但年/月/日对读屏是 radio group 里的三个
     radio。与 GTK 侧一致——整件控件是一个 tab stop（bar 可聚焦），三段自己不进
     Tab 序列，靠左右方向键切换；指示条是 presentation，不进无障碍树。 */
  function applyModeSelectorA11y() {
    qa(".mode-selector").forEach(function (bar) {
      bar.setAttribute("role", "radiogroup");
      bar.setAttribute("aria-label", tr("photo.mode.group"));
      qa(".segment", bar).forEach(function (s) {
        s.setAttribute("role", "radio");
        s.setAttribute("aria-checked", s.classList.contains("active") ? "true" : "false");
        // 与 set_labels_i18n 一致：无障碍名是显式下发的同一个字符串，而不是靠读屏
        // 自己从子节点里捞文本。
        s.setAttribute("aria-label", (s.textContent || "").trim());
      });
      var dot = q(".mode-dot", bar);
      if (dot) dot.setAttribute("role", "presentation");
    });
  }

  /* P2-4 已落盘，所以这里是常态而不是提案开关：格子的名字来自绑定它的代码
     （真实实现 SquareTile::set_accessible_name(item.display_name())），徽标与图标
     各报自己的状态，纯装饰的东西从树里摘掉。落盘前形态见 data-pv-demo=tiles-anonymous。
     名字逐字取仓库 i18n 键，跟界面语言走，不另造同义词。 */
  var A11Y_TARGETS = ".button.bare[data-act], .button.bare[data-nav], .button.round[data-ui], " +
    ".button.danger[data-act], .tile .badge, img.viewer-sync-badge, .multi-only";
  // 与真实实现同构的装饰图标：旁边的标题/标签已经说完这句话，图标进树只会重复
  // 一遍或以「未命名图片」的形式多出一条噪音（overview_sync_icon、media_error_icon、
  // AdwStatusPage 的图标、侧栏箭头都是这一类）。
  var DECORATIVE = ".status-icon";
  function iconLabel(el) {
    var c = el.classList;
    var text = (el.textContent || "").trim();
    if (c.contains("duration")) return tr("tile.badge.duration").replace("{duration}", text);
    if (c.contains("motion")) return tr("tile.badge.motion");
    if (c.contains("favorite")) return tr("tile.badge.favorite");
    if (c.contains("synced") || c.contains("viewer-sync-badge")) return tr("sync.badge.synced");
    var named = el.closest("[data-name]");
    var base = named ? named.getAttribute("data-name") : "";
    return el.dataset.key ? base + "（" + el.dataset.key + "）" : base;
  }
  function applyIconLabels() {
    // 落盘前：格子匿名、徽标只念出 glyph、对勾被当成状态朗读者。
    var on = body.dataset.pvDemo !== "tiles-anonymous";
    qa(DECORATIVE).forEach(function (el) {
      if (on) el.setAttribute("aria-hidden", "true");
      else el.removeAttribute("aria-hidden");
    });
    qa(A11Y_TARGETS).forEach(function (el) {
      if (el.dataset.pvLabelled === "1") {
        if (el.dataset.pvLabelledTitle === "1") el.removeAttribute("title");
        el.removeAttribute("aria-label");
        el.removeAttribute("data-pv-labelled");
        el.removeAttribute("data-pv-labelled-title");
      }
      if (!on) return;
      // 真实代码里多数图标按钮已经有 set_tooltip_text（viewer_page.rs:473-476），
      // 那种情况只补 accessible name，不去覆盖 tooltip。
      var hadTitle = el.hasAttribute("title");
      var label = hadTitle ? el.getAttribute("title") : iconLabel(el);
      if (!label) return;
      if (!hadTitle) {
        el.setAttribute("title", label);
        el.dataset.pvLabelledTitle = "1";
      }
      el.setAttribute("aria-label", label);
      el.dataset.pvLabelled = "1";
    });
    // 格子本体：名字就是用户看得见的文件名。原型里 .tile 自己是命名总览的
    // hotspot（data-name 是「视频宫格」这类控件标签），所以文件名写到内层
    // role=img 节点上，等价于真实实现里 tile 本体承担 img 角色。
    qa(".tile[data-media]").forEach(function (tile) {
      var face = q(".pv-a11y-face", tile);
      var note = q(".pv-a11y-note", tile);
      if (on) {
        if (note) note.remove();
        if (!face) {
          face = document.createElement("span");
          face.className = "pv-a11y-face";
          face.setAttribute("role", "img");
          tile.insertBefore(face, tile.firstChild);
        }
        face.setAttribute("aria-label", mediaName(tile));
        return;
      }
      if (face) face.remove();
      if (!note) {
        note = document.createElement("span");
        note.className = "pv-a11y-note";
        tile.appendChild(note);
      }
      var badge = q(".badge", tile);
      note.textContent = "读屏：无名称" +
        (badge ? " · 角标念成「" + (badge.textContent || "").trim() + "」" : "");
    });
    // 对勾在真实实现里是 presentation：它常驻树中、只用透明度淡入淡出。
    qa(".tile .checkmark").forEach(function (tick) {
      if (on) tick.setAttribute("aria-hidden", "true");
      else tick.removeAttribute("aria-hidden");
    });
  }

  /* ------------------------------------------------------------- 查看器 */

  var viewer = { tiles: [], index: 0, scale: 1, panX: 0, panY: 0, rot: 0 };
  var ZOOM_MIN = 1.0, ZOOM_MAX = 8.0, ZOOM_STEP = 1.25;

  function openViewer(tiles, index) {
    viewer.tiles = tiles;
    viewer.index = index;
    go("viewer");
    renderViewer();
    revealStageHint(false);
  }

  /* P1-8：真实代码已落盘；原型仍保留对照开关。自动收起属于后续沉浸浏览项。 */
  var stageHint = { seen: false, timer: 0 };
  function revealStageHint(force) {
    var el = q(".stage-hint");
    if (!el) return;
    clearTimeout(stageHint.timer);
    if (!enabled("p1-8") || current !== "viewer") {
      stageHint.seen = false;
      el.classList.remove("is-shown", "is-dismissed");
      return;
    }
    if (stageHint.seen && !force) return;
    stageHint.seen = true;
    el.classList.remove("is-dismissed");
    el.classList.add("is-shown");
  }
  function stepViewer(delta) {
    var next = viewer.index + delta;
    if (next < 0 || next >= viewer.tiles.length) {
      renderViewerChrome();
      return;
    }
    viewer.index = next;
    resetTransform();
    renderViewer();
  }
  function resetTransform() {
    viewer.scale = 1; viewer.panX = 0; viewer.panY = 0;
    applyTransform();
  }
  function stepZoom(dir) {
    var s = viewer.scale;
    s = dir > 0 ? Math.min(ZOOM_MAX, s * ZOOM_STEP) : Math.max(ZOOM_MIN, s / ZOOM_STEP);
    if (s === 1) { viewer.panX = 0; viewer.panY = 0; }
    viewer.scale = s;
    applyTransform();
  }
  function applyTransform() {
    var photo = q("#viewer-photo");
    var media = q(".viewer-media");
    if (photo) {
      photo.style.transform = "translate(" + viewer.panX + "px," + viewer.panY + "px) scale(" +
        viewer.scale + ") rotate(" + viewer.rot + "deg)";
    }
    if (media) {
      media.classList.toggle("is-pannable", enabled("p1-8") && viewer.scale > 1.0001);
    }
    var zoom = q("#viewer-zoom");
    if (zoom) {
      zoom.textContent = Math.round(viewer.scale * 100) + "%";
      var host = q(".zoom-level");
      if (host) host.classList.toggle("is-zoomed", viewer.scale > 1.0001);
    }
  }
  function clampPan() {
    var media = q(".viewer-media");
    if (!media) return;
    var rect = media.getBoundingClientRect();
    var limitX = Math.max(0, (rect.width * (viewer.scale - 1)) / 2);
    var limitY = Math.max(0, (rect.height * (viewer.scale - 1)) / 2);
    viewer.panX = Math.min(limitX, Math.max(-limitX, viewer.panX));
    viewer.panY = Math.min(limitY, Math.max(-limitY, viewer.panY));
  }
  function renderViewer() {
    var tile = viewer.tiles[viewer.index];
    if (!tile) return;
    var broken = tile.classList.contains("broken");
    var photo = q("#viewer-photo");
    if (photo) {
      photo.src = bigFor(tile);
      photo.style.display = broken ? "none" : "block";
    }
    var err = q(".media-error");
    if (err) err.classList.toggle("is-open", broken);
    var name = mediaName(tile);
    setText("#viewer-filename", name);
    setText("#detail-name", name);
    setText("#viewer-date", "2026年7月" + (20 - (viewer.index % 12)) + "日");
    setText("#detail-date", "2026年7月" + (20 - (viewer.index % 12)) + "日");
    setText("#detail-type", isVideo(tile) ? "video/mp4" : "image/jpeg");
    setText("#detail-dims", isVideo(tile) ? "1920×1080" : "4032×3024");
    setText("#detail-size", (2.4 + viewer.index * 0.37).toFixed(1) + " MB");
    setText("#detail-folder", "/photos/2026");
    resetTransform();
    renderViewerChrome();
    qa(".film-thumb").forEach(function (t, i) {
      t.classList.toggle("active", i === viewer.index % 17);
    });
  }
  function renderViewerChrome() {
    var total = viewer.tiles.length;
    var pos = q("#viewer-position");
    if (pos) {
      pos.textContent = total ? (viewer.index + 1) + " / " + total : "1 / 17";
      pos.style.visibility = !total && body.dataset.pvMode === "run" ? "hidden" : "";
    }
    var prev = q("#viewer-prev"), next = q("#viewer-next");
    if (prev) prev.disabled = !total || viewer.index === 0;
    if (next) next.disabled = !total || viewer.index >= total - 1;
  }
  function setText(sel, value) { var el = q(sel); if (el) el.textContent = value; }

  /* ---------------------------------------------------------- 详情/编辑 */

  var editor = { dirty: false, compare: false, values: {} };

  function toggleDetails() {
    var panel = q(".side-panel.details");
    if (!panel) return;
    panel.classList.toggle("open");
  }
  function toggleEditor() {
    var panel = q(".side-panel.editor");
    if (!panel) return;
    var open = !panel.classList.contains("open");
    panel.classList.toggle("open", open);
    body.dataset.pvEditing = open ? "1" : "0";
  }
  function setDirty(value) {
    editor.dirty = !!value;
    body.dataset.pvDirty = editor.dirty ? "1" : "0";
  }
  function markDirty(value) {
    // 无参调用 = 标记为脏（编辑面板的既有调用点）；显式传 false 用于评审脚本复位。
    setDirty(value !== false);
    applyEditFilter();
  }
  function applyEditFilter() {
    var photo = q("#viewer-photo");
    if (!photo) return;
    if (editor.compare || !editor.dirty) {
      photo.style.filter = "none";
      return;
    }
    var b = editor.values.brightness || 0, c = editor.values.contrast || 0, s = editor.values.saturation || 0;
    photo.style.filter = "brightness(" + (1 + b) + ") contrast(" + (1 + c) + ") saturate(" + (1 + s) + ")";
  }
  function attemptCloseEditor() {
    function reallyClose() {
      setDirty(false);
      editor.values = {};
      editor.compare = false;
      var panel = q(".side-panel.editor");
      if (panel) panel.classList.remove("open");
      body.dataset.pvEditing = "0";
      applyEditFilter();
      qa(".scale[data-adjust]").forEach(function (sc) {
        var fill = q(".scale-fill", sc);
        if (fill) fill.style.width = "0%";
      });
    }
    if (editor.dirty) {
      var dlg = q("#discard-dialog");
      if (dlg) dlg.classList.add("is-open");
      return;
    }
    reallyClose();
  }

  /* -------------------------------------------------------------- 搜索 */

  var SEARCH_INDEX = null;
  function searchIndex() {
    if (SEARCH_INDEX) return SEARCH_INDEX;
    SEARCH_INDEX = qa('.screen[data-screen="search"] .result-section .tile[data-media]').map(function (t, i) {
      return { name: mediaName(t), date: mediaDate(t, i), video: isVideo(t), tile: t };
    });
    return SEARCH_INDEX;
  }
  function searchField() {
    var active = q('.screen[data-screen="search"] .field.active');
    return active ? active.dataset.field : "all";
  }
  function match(item, query, field) {
    var name = item.name.toLowerCase().indexOf(query.toLowerCase()) >= 0;
    var parts = item.date.match(/(\d{4})年(\d{1,2})月(\d{1,2})日/);
    var date = parts[1] + parts[2].padStart(2, "0") + parts[3].padStart(2, "0");
    var dateQuery = query.replace(/[^\d]/g, "");
    return ((field === "all" || field === "name") && name) ||
      ((field === "all" || field === "date") && /\d/.test(query) && date.indexOf(dateQuery) >= 0);
  }
  function searchHits(query) {
    var field = searchField();
    return searchIndex().filter(function (item) { return match(item, query, field); });
  }
  function renderSearchResults(hits) {
    qa('.screen[data-screen="search"] .result-section').forEach(function (section) {
      var video = section.dataset.kind === "video";
      var matching = hits.filter(function (item) { return item.video === video; });
      section.style.display = matching.length ? "" : "none";
      qa('.tile[data-media]', section).forEach(function (tile) {
        var index = matching.findIndex(function (item) { return item.tile === tile; });
        tile.style.display = index >= 0 && index < (video ? 1 : 4) ? "" : "none";
      });
      var more = q(".more-tile", section);
      if (more) more.style.display = matching.length > (video ? 1 : 4) ? "" : "none";
    });
  }
  function openSearchMore(section) {
    var query = ((q("#search-input") || {}).value || "").trim();
    var video = section.dataset.kind === "video";
    var results = searchHits(query).filter(function (item) { return item.video === video; });
    var grid = q("#search-more-grid");
    grid.replaceChildren();
    results.forEach(function (item) {
      var tile = item.tile.cloneNode(true);
      tile.removeAttribute("data-ui");
      tile.removeAttribute("data-name");
      tile.removeAttribute("data-impl");
      tile.removeAttribute("data-props");
      tile.style.display = "";
      grid.appendChild(tile);
    });
    applyImages();
    setText("#search-more-title", tr(video ? "search.videos" : "search.images"));
    go("search-more");
  }
  var searchTimer = null;
  var searchBusyTimer = null;
  var searchLatencyMs = 200;
  function setSearchLatency(ms) { searchLatencyMs = ms; runSearch((q("#search-input") || {}).value); }
  function runSearch(raw) {
    var query = (raw || "").trim();
    clearTimeout(searchTimer); clearTimeout(searchBusyTimer);
    if (!query) {
      renderSearchResults([]);
      body.dataset.pvSearch = "idle";
      return;
    }
    if (body.dataset.pvDemo === "search-blank") {
      var beforeHits = searchHits(query);
      renderSearchResults(beforeHits);
      body.dataset.pvSearch = beforeHits.length ? "results" : "idle";
      return;
    }
    searchBusyTimer = setTimeout(function () { body.dataset.pvSearch = "busy"; }, 300);
    searchTimer = setTimeout(function () {
      clearTimeout(searchBusyTimer);
      var hits = searchHits(query);
      var title = q("#search-none-title");
      if (title) title.textContent = tr("empty.search_none.title").replace("{query}", query);
      renderSearchResults(hits);
      body.dataset.pvSearch = hits.length ? "results" : "none";
    }, searchLatencyMs);
  }

  /* ------------------------------------------------------------ i18n */

  // 已存在的键与值逐字取自仓库 i18n/zh-CN.json 与 i18n/en.json，不另造同义词；
  // 标了「提案新增」的键是方案要引入的，仓库里当前查不到。
  var I18N = {
    "zh": {
      "search.title": "搜索", "search.placeholder": "按文件名或拍摄时间搜索",
      "search.images": "图片", "search.videos": "视频", "search.more": "更多",
      "viewer.tooltip.previous": "上一张", "viewer.tooltip.next": "下一张",
      "viewer.tooltip.zoom_in": "放大", "viewer.tooltip.zoom_out": "缩小",
      "viewer.tooltip.zoom_reset": "还原缩放",
      "viewer.tooltip.rotate_left": "向左旋转", "viewer.tooltip.rotate_right": "向右旋转",
      "viewer.tooltip.fullscreen": "全屏预览", "viewer.tooltip.favorite": "收藏",
      "viewer.tooltip.edit": "编辑", "viewer.tooltip.move_to_trash": "移入回收站",
      "viewer.details.title": "详情",
      "app.title": "照片查看器", "window.sidebar": "图库",
      "sidebar.photos": "照片", "sidebar.albums": "相册", "sidebar.media_types": "媒体类型",
      "sidebar.trash": "回收站", "sidebar.settings": "设置",
      "page.photos.title": "照片", "page.albums.title": "相册", "page.trash.title": "回收站",
      "album_picker.title": "添加到相册",
      // 侧栏的智能相册行名；「图片」「收藏」与上面的键同字，回查表以这里为准。
      "album.images.name": "图片", "album.videos.name": "视频", "album.motion_photos.name": "动态图片",
      "album.animated.name": "动图", "album.hdr.name": "HDR", "album.favorites.name": "收藏",
      "album.no_albums_yet": "还没有相册",
      // 提案新增：P1-6 选择计数（仍未落盘）。
      "photos.selection.count": "已选择 {n} 项", "photos.selection.limit": "（已达上限）",
      "photo.mode.group": "照片分组方式",
      "photo.mode.year": "年", "photo.mode.month": "月", "photo.mode.day": "日",
      // P2-4 已落盘：以下 4 个键逐字取自仓库 i18n/zh-CN.json:30,189-191，
      // 徽标的 accessible name 与真实代码用同一批键。
      "sync.badge.synced": "已同步到云端", "tile.badge.motion": "动态图片",
      "tile.badge.favorite": "已收藏", "tile.badge.duration": "视频时长 {duration}",
      // P0-5 已落盘：以下 8 个键逐字取自仓库 i18n/zh-CN.json，
      // 三个字段标签在真实代码里由 search_page.rs:198-200 填入，模板不再硬编码。
      "search.field.all": "全部", "search.field.name": "文件名", "search.field.date": "日期",
      "empty.search_idle.title": "搜索你的图库",
      "empty.search_idle.description": "输入文件名或拍摄日期（YYYY/MM/DD）开始搜索。",
      "empty.search_none.title": "没有找到「{query}」",
      "empty.search_none.description": "换一个关键词，或将搜索字段切回「全部」。",
      "empty.search_none.clear": "清除搜索",
      // P2-8 已落盘：以下四键逐字取自仓库 i18n/zh-CN.json:168-169, 174, 178, 274。
      "photos.overview.show": "展开图库概览",
      "photos.overview.hide": "收起图库概览",
      "photos.overview.sync.paused": "同步已暂停；首页下拉后继续同步",
      "photos.overview.sync.failed": "同步失败；可点「重试」，或在设置中查看详情",
      "common.retry": "重试",
      // P0-4 已落盘：以下 34 个键来自仓库 i18n/zh.json，快捷键表与设置行直接引用。
      "keyboard.window.title": "键盘快捷键", "keyboard.group.global": "全局", "keyboard.group.browsing": "浏览与选择",
      "keyboard.group.viewer": "图片查看", "keyboard.show_shortcuts": "显示键盘快捷键", "keyboard.cancel_or_close": "取消或关闭",
      "keyboard.navigate_back": "返回上一级", "keyboard.search": "搜索照片", "keyboard.settings": "打开设置",
      "keyboard.move_focus_up": "焦点上移", "keyboard.move_focus_down": "焦点下移", "keyboard.move_focus_left": "焦点左移",
      "keyboard.move_focus_right": "焦点右移", "keyboard.open_focused": "打开焦点项",
      "keyboard.toggle_selection": "选中或取消当前项", "keyboard.select_all": "全选", "keyboard.move_to_trash": "移入回收站",
      "keyboard.previous_media": "上一张", "keyboard.next_media": "下一张", "keyboard.close_viewer": "关闭查看器",
      "keyboard.toggle_playback": "播放或暂停", "keyboard.zoom_in": "放大", "keyboard.zoom_out": "缩小",
      "keyboard.zoom_reset": "恢复原始大小", "keyboard.rotate_right": "向右旋转", "keyboard.rotate_left": "向左旋转",
      "keyboard.fullscreen_preview": "全屏预览", "keyboard.toggle_details": "显示或隐藏详细信息",
      "keyboard.toggle_edit": "打开或关闭编辑面板", "keyboard.toggle_favorite": "标记或取消收藏", "setting.section.keyboard": "键盘",
      "setting.section.keyboard_description": "所有快捷键都可以直接用键盘完成当前页面的操作。", "setting.keyboard.reference": "查看快捷键列表",
      "setting.keyboard.reference_description": "随时按 F1 或 Ctrl+/ 打开"
    },
    "en": {
      "search.title": "Search", "search.placeholder": "Search by file name or shooting date",
      "search.images": "Images", "search.videos": "Videos", "search.more": "More",
      "viewer.tooltip.previous": "Previous", "viewer.tooltip.next": "Next",
      "viewer.tooltip.zoom_in": "Zoom In", "viewer.tooltip.zoom_out": "Zoom Out",
      "viewer.tooltip.zoom_reset": "Reset Zoom",
      "viewer.tooltip.rotate_left": "Rotate Left", "viewer.tooltip.rotate_right": "Rotate Right",
      "viewer.tooltip.fullscreen": "Fullscreen Preview", "viewer.tooltip.favorite": "Favorite",
      "viewer.tooltip.edit": "Edit", "viewer.tooltip.move_to_trash": "Move to Trash",
      "viewer.details.title": "Details",
      "app.title": "Photo Viewer", "window.sidebar": "Library",
      "sidebar.photos": "Photos", "sidebar.albums": "Albums", "sidebar.media_types": "Media Types",
      "sidebar.trash": "Trash", "sidebar.settings": "Settings",
      "page.photos.title": "Photos", "page.albums.title": "Albums", "page.trash.title": "Trash",
      "album_picker.title": "Add to Album",
      "album.images.name": "Photos", "album.videos.name": "Videos", "album.motion_photos.name": "Live Photos",
      "album.animated.name": "Animated", "album.hdr.name": "HDR", "album.favorites.name": "Favorites",
      "album.no_albums_yet": "No Albums Yet",
      "photos.selection.count": "{n} selected", "photos.selection.limit": " (limit reached)",
      "photo.mode.group": "Photo grouping",
      "photo.mode.year": "Year", "photo.mode.month": "Month", "photo.mode.day": "Day",
      // P2-4 已落盘：与仓库 i18n/en.json 同值（:30, :189-191）。
      "sync.badge.synced": "Synced to cloud", "tile.badge.motion": "Live photo",
      "tile.badge.favorite": "Favorited", "tile.badge.duration": "Video duration {duration}",
      "search.field.all": "All", "search.field.name": "File name", "search.field.date": "Date",
      "empty.search_idle.title": "Search Your Library",
      "empty.search_idle.description": "Type a file name or a shooting date (YYYY/MM/DD) to start.",
      "empty.search_none.title": 'No results for "{query}"',
      "empty.search_none.description": "Try a different term, or switch the search field back to All.",
      "empty.search_none.clear": "Clear Search",
      // P2-8 已落盘：与仓库 i18n/en.json:168-169, 174, 178, 274 同值。
      "photos.overview.show": "Show library overview",
      "photos.overview.hide": "Hide library overview",
      "photos.overview.sync.paused": "Sync is paused; pull down on Photos to sync",
      "photos.overview.sync.failed": "Sync failed; use Retry, or open Settings for details",
      "common.retry": "Retry",
      // P0-4 已落盘：以下 34 个键来自仓库 i18n/en.json，快捷键表与设置行直接引用。
      "keyboard.window.title": "Keyboard Shortcuts", "keyboard.group.global": "Global",
      "keyboard.group.browsing": "Browsing & Selection", "keyboard.group.viewer": "Photo Viewer",
      "keyboard.show_shortcuts": "Show keyboard shortcuts", "keyboard.cancel_or_close": "Cancel or close",
      "keyboard.navigate_back": "Go back", "keyboard.search": "Search photos",
      "keyboard.settings": "Open Settings", "keyboard.move_focus_up": "Move focus up",
      "keyboard.move_focus_down": "Move focus down", "keyboard.move_focus_left": "Move focus left",
      "keyboard.move_focus_right": "Move focus right", "keyboard.open_focused": "Open the focused item",
      "keyboard.toggle_selection": "Select or deselect the current item", "keyboard.select_all": "Select all",
      "keyboard.move_to_trash": "Move to Trash", "keyboard.previous_media": "Previous photo",
      "keyboard.next_media": "Next photo", "keyboard.close_viewer": "Close the viewer",
      "keyboard.toggle_playback": "Play or pause", "keyboard.zoom_in": "Zoom in", "keyboard.zoom_out": "Zoom out",
      "keyboard.zoom_reset": "Reset zoom", "keyboard.rotate_right": "Rotate right",
      "keyboard.rotate_left": "Rotate left", "keyboard.fullscreen_preview": "Fullscreen preview",
      "keyboard.toggle_details": "Show or hide details", "keyboard.toggle_edit": "Show or hide the edit panel",
      "keyboard.toggle_favorite": "Toggle favorite", "setting.section.keyboard": "Keyboard",
      "setting.section.keyboard_description": "Every action on the current page can also be done with the keyboard.",
      "setting.keyboard.reference": "View the shortcut list",
      "setting.keyboard.reference_description": "Press F1 or Ctrl+/ any time"
    }
  };
  function locale() { return body.dataset.pvLocale || "zh"; }
  function tr(key) { return (I18N[locale()] || I18N.zh)[key] || key; }

  // 侧栏行与页标题在真实代码里同样走 tr()，但它们在 7 个屏幕里重复出现；
  // 这里用中文原文回查键名，而不是把 data-i18n 抄几十遍。首次命中后记住键。
  var ZH_TO_KEY = {};
  Object.keys(I18N.zh).forEach(function (k) {
    var v = I18N.zh[k];
    if (v.indexOf("{") < 0) ZH_TO_KEY[v] = k;
  });
  var LOCALIZED_TEXT = ".row > span:not(.icon):not(.count), .header-title";
  function localizeText(el) {
    var key = el.dataset.pvTextKey;
    if (!key) {
      var text = (el.textContent || "").trim();
      key = ZH_TO_KEY[text];
      if (key) el.dataset.pvTextKey = key;
    }
    if (key) el.textContent = tr(key);
  }

  function applyLocale() {
    qa(LOCALIZED_TEXT).forEach(localizeText);
    qa("[data-i18n]").forEach(function (el) {
      var key = el.dataset.i18n;
      // data-i18n-hardcoded：字符串仍写死在模板里的历史形态（search-page.blp:36-47 旧版），
      // 只在「落盘前」演示态复现——P0-5 之后真实代码走 tr()，英文界面显示英文标签。
      if (el.hasAttribute("data-i18n-hardcoded") && body.dataset.pvDemo === "search-blank") {
        el.textContent = I18N.zh[key];
        return;
      }
      el.textContent = tr(key);
    });
    // 查看器 tooltip 在真实代码里已经过 tr()（viewer_page.rs:485），所以两种语言都要翻；
    // 键名后缀是 P0-4 落盘行为（tooltip_with_key），常态存在，只在「落盘前」演示态里摘掉。
    // baseTitle 只认一次，保证本函数可重复执行。
    // 无障碍名与可见文案同源（mode_selector.rs 的 set_labels_i18n 一次写两者），
    // 所以换语言时必须一起翻，否则读屏念的是另一种语言。
    applyModeSelectorA11y();
    qa("[data-i18n-title], [data-key]").forEach(function (el) {
      if (el.dataset.baseTitle === undefined) el.dataset.baseTitle = el.getAttribute("title") || "";
      var base = el.dataset.i18nTitle ? tr(el.dataset.i18nTitle) : el.dataset.baseTitle;
      if (!base) return;
      var key = el.dataset.key;
      var blind = body.dataset.pvDemo === "shortcut-blind";
      el.setAttribute("title", key && !blind ? base + " (" + key + ")" : base);
    });
    var input = q("#search-input");
    if (input) input.placeholder = tr("search.placeholder");
    // 零结果标题回显关键词，所以只能由搜索流程写入；这里跟着语言重算一次，
    // 与真实实现一致（set_search_state 每次显示前刷新标题）。
    var noneTitle = q("#search-none-title");
    var noneQuery = ((input && input.value) || "").trim();
    if (noneTitle && noneQuery) {
      noneTitle.textContent = tr("empty.search_none.title").replace("{query}", noneQuery);
    }
    // 快捷键表里的字形来自 gtk_accelerator_get_label，方向键与空格跟随界面语言，
    // 所以中文「上/下/左/右/空格」、英文 Up/Down/Left/Right/Space。
    qa("kbd[data-kbd-en]").forEach(function (el) {
      if (el.dataset.baseKbd === undefined) el.dataset.baseKbd = el.textContent;
      el.textContent = locale() === "en" ? el.dataset.kbdEn : el.dataset.baseKbd;
    });
    updateCount();
    // P2-4 的 accessible name 取自 tooltip 文案，语言一变就要重算。
    applyIconLabels();
    // P2-8 同理：chevron 的名字与 glyph 都随展开状态重算。
    applyOverview();
  }

  /* ---------------------------------------------------- 图库概览（P2-8） */

  // P2-8 已落盘，所以这里是常态而不是提案开关。chevron 在真实实现里是
  // overview_revealer 的镜像（photos_page.rs apply_overview_disclosure_state 挂在
  // notify::reveal-child 上），所以 glyph、tooltip、aria-expanded 都由那一个状态派生；
  // 重试只在 Failed 出现（apply_overview_retry_affordance）。运行模式的显隐规则在
  // styles.css，命名模式两件都恒可见，因为它们是 data-ui 条目。
  function overviewIsOpen() {
    return body.dataset.pvOvr === "open" || body.dataset.pvOvr === "failed";
  }
  function setOverview(state) {
    body.dataset.pvOvr = state;
    applyOverview();
    emit("overview", state);
  }
  function applyOverview() {
    var open = overviewIsOpen();
    var btn = q(".overview-disclosure");
    if (btn) {
      var name = tr(open ? "photos.overview.hide" : "photos.overview.show");
      btn.textContent = open ? "⌃" : "⌄";
      btn.setAttribute("title", name);
      btn.setAttribute("aria-label", name);
      btn.setAttribute("aria-expanded", open ? "true" : "false");
    }
    var retry = q(".overview-retry");
    if (retry) retry.textContent = tr("common.retry");
    var sync = q('[data-ovr="sync"]');
    if (sync) {
      sync.textContent = tr(body.dataset.pvOvr === "failed"
        ? "photos.overview.sync.failed" : "photos.overview.sync.paused");
    }
  }

  /* -------------------------------------------------------- 扫描状态 */

  // P0-1 已落盘：三态占位页是真实 chrome，只看 body[data-pv-scan]，不需要提案开关。
  // 落盘的扫描中页面只有 spinner 与固定文案，没有实时计数，所以这里不跑定时器。
  function setScan(state) {
    body.dataset.pvScan = state;
    emit("scan", state);
  }

  /* ------------------------------------------------------- 环境开关 */

  function setMode(mode) {
    body.dataset.pvMode = mode;
    if (mode === "run") {
      go(current, { push: false });
    } else {
      qa(".screen").forEach(function (s) { s.classList.remove("is-active"); });
    }
    emit("mode", mode);
    renderViewerChrome();
    emit("routes");
  }
  function setTransparency(value) {
    var t = Math.max(0, Math.min(100, Number(value) || 0)) / 100;
    body.style.setProperty("--t", t.toFixed(3));
    body.classList.toggle("pv-transparency", t > 0);
    body.dataset.pvTransparency = String(Math.round(t * 100));
    // 深链/批次预设直接调本函数，界面里的滑杆要跟着反映状态。
    var range = q("#transparency-range");
    if (range && range !== document.activeElement) range.value = String(Math.round(t * 100));
    emit("transparency", Math.round(t * 100));
  }
  function setMotion(off) {
    body.dataset.pvMotion = off ? "off" : "auto";
    var sw = q('[data-act="toggle-motion"]');
    if (sw) sw.classList.toggle("on", off);
  }
  function setMaterial(kind) {
    body.dataset.pvMaterial = kind;
    var sw = q('[data-act="toggle-material"]');
    if (sw) sw.classList.toggle("off", kind === "plain");
  }
  function setWidth(kind) { body.dataset.pvWidth = kind; emit("width", kind); }
  function setLocale(kind) { body.dataset.pvLocale = kind; applyLocale(); emit("locale", kind); }

  /* --------------------------------------------------- 对比度测量 */

  function channel(v) {
    var c = v / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  }
  function luminance(rgb) {
    return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
  }
  function parseColor(str) {
    var m = /rgba?\(([^)]+)\)/.exec(str || "");
    if (!m) return null;
    var parts = m[1].split(",").map(function (p) { return parseFloat(p); });
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
  }
  function over(fg, bg) {
    var a = fg.a * (1 + (1 - fg.a) * 0);
    return {
      r: fg.r * a + bg.r * (1 - a),
      g: fg.g * a + bg.g * (1 - a),
      b: fg.b * a + bg.b * (1 - a),
      a: 1
    };
  }
  function backdropOf(el) {
    var node = el;
    var acc = null;
    while (node && node !== document.documentElement) {
      var c = parseColor(getComputedStyle(node).backgroundColor);
      if (c && c.a > 0) {
        if (!acc) acc = c;
        else acc = over(c, acc);
        if (acc.a >= 0.999) break;
      }
      node = node.parentElement;
    }
    if (!acc) acc = { r: 21, g: 23, b: 29, a: 1 };
    return acc;
  }
  function ratioOf(fgRgb, bgRgb) {
    var l1 = luminance([fgRgb.r, fgRgb.g, fgRgb.b]);
    var l2 = luminance([bgRgb.r, bgRgb.g, bgRgb.b]);
    var hi = Math.max(l1, l2), lo = Math.min(l1, l2);
    return (hi + 0.05) / (lo + 0.05);
  }
  var CONTRAST_TARGETS = [
    { sel: ".row .count", label: "侧栏计数", target: 4.5 },
    { sel: ".stats-line", label: "库统计行", target: 4.5 },
    { sel: ".pref-row > span:last-child", label: "设置值文本", target: 4.5 },
    { sel: ".status-page small", label: "空态描述", target: 4.5 },
    { sel: ".more-tile", label: "搜索「更多」瓦片", target: 4.5 },
    { sel: ".footer", label: "页脚说明", target: 4.5 }
  ];
  function contrastReport() {
    return CONTRAST_TARGETS.map(function (row) {
      // 只量真正渲染在屏幕上的那个节点：隐藏界面里的元素取不到有效背景色，
      // 报出来的比值是噪声，评审时会被当成结论。
      var el = qa(row.sel).filter(function (n) { return n.getClientRects().length > 0; })[0];
      var screen = el && el.closest(".screen") ? el.closest(".screen").dataset.screen : null;
      var cs = el && getComputedStyle(el);
      var fg = cs && parseColor(cs.color);
      if (!el || !fg) {
        return { label: row.label, sel: row.sel, screen: screen, skipped: true, target: row.target };
      }
      var bg = backdropOf(el.parentElement || el);
      var composited = over(fg, bg);
      var r = ratioOf(composited, bg);
      return {
        label: row.label,
        sel: row.sel,
        screen: screen,
        alpha: Number(fg.a.toFixed(2)),
        ratio: Number(r.toFixed(2)),
        target: row.target,
        pass: r >= row.target
      };
    });
  }

  /* --------------------------------------------------------- 事件接线 */

  function wireNavigation() {
    document.addEventListener("click", function (ev) {
      var nav = ev.target.closest("[data-nav]");
      if (nav) {
        ev.preventDefault();
        ev.stopPropagation();
        var target = nav.dataset.nav;
        if (target === "album") {
          var album = ALBUMS[nav.dataset.album] || ALBUMS.trip;
          setText("#album-title", album.title);
        }
        go(target);
        return;
      }
      var act = ev.target.closest("[data-act]");
      if (act) { runAction(act.dataset.act, act, ev); return; }
      var seg = ev.target.closest(".segment[data-mode]");
      if (seg) { selectSegment(seg); return; }
      var field = ev.target.closest(".field[data-field]");
      if (field) {
        qa(".field", field.closest(".field-bar")).forEach(function (f) {
          f.classList.toggle("active", f === field);
        });
        runSearch((q("#search-input") || {}).value);
        return;
      }
      var thumb = ev.target.closest(".film-thumb[data-media]");
      if (thumb && body.dataset.pvMode === "run") {
        var idx = qa(".film-thumb").indexOf(thumb);
        if (idx >= 0 && idx < viewer.tiles.length) { viewer.index = idx; resetTransform(); renderViewer(); }
      }
    });

    document.addEventListener("contextmenu", function (ev) {
      var tile = ev.target.closest(".tile[data-media]");
      if (!tile || body.dataset.pvMode !== "run" ||
          (tile.closest('.screen[data-screen="search"]') && !enabled("p1-15"))) return;
      ev.preventDefault();
      var menu = q("#context-menu");
      if (!menu) return;
      menu.style.left = Math.min(ev.clientX, window.innerWidth - 190) + "px";
      menu.style.top = Math.min(ev.clientY, window.innerHeight - 160) + "px";
      menu.classList.add("is-open");
    });
    document.addEventListener("pointerdown", function (ev) {
      var menu = q("#context-menu");
      if (menu && menu.classList.contains("is-open") && !ev.target.closest("#context-menu")) {
        menu.classList.remove("is-open");
      }
    });

    document.addEventListener("click", function (ev) {
      if (body.dataset.pvMode !== "run") return;
      var more = ev.target.closest(".screen[data-screen='search'] .more-tile");
      if (more) {
        openSearchMore(more.closest(".result-section"));
        return;
      }
      var tile = ev.target.closest(".screen.is-active .tile[data-media]");
      if (!tile) return;
      if (tile.closest('.screen[data-screen="search"]') && multi && !enabled("p1-15")) return;
      var grid = tile.closest(".tile-grid");
      var tiles = qa('.tile[data-media]', grid).filter(function (item) {
        return getComputedStyle(item).display !== "none";
      });
      if (multi) toggleTile(tile);
      else openViewer(tiles, tiles.indexOf(tile));
    });
  }

  function runAction(act, el, ev) {
    switch (act) {
      // P0-5 已落盘：零结果页的「清除搜索」清空输入框、把焦点交回搜索框并回到查询前态。
      case "search-clear":
        var searchInput = q("#search-input");
        if (searchInput) { searchInput.value = ""; searchInput.focus(); }
        runSearch("");
        break;
      case "search-more-back": back(); break;
      case "enter-multi": setMulti(true); toast("已进入多选：点击语义从「打开」翻转为「切换选中」", { kind: "info", ms: 2600 }); break;
      case "exit-multi": setMulti(false); break;
      case "select-all": selectAllInScreen(); break;
      case "batch-fav":
        toast("已收藏 " + selectedIds().length + " 项", {
          kind: "success",
          action: { label: "撤销", run: function () { toast("已撤销收藏", { kind: "info" }); } }
        });
        break;
      case "batch-trash":
        var moved = selectedIds().length;
        toast("已移到回收站（" + moved + " 项）", {
          kind: "success",
          action: { label: "撤销", run: function () { toast("已还原 " + moved + " 项", { kind: "info" }); } }
        });
        setMulti(false);
        break;
      case "rebuild": simulateRebuild(); break;
      case "viewer-prev": stepViewer(-1); break;
      case "viewer-next": stepViewer(1); break;
      case "viewer-fav":
        var fav = q('[data-act="viewer-fav"]');
        var on = fav.textContent.trim() === "♥";
        fav.textContent = on ? "♡" : "♥";
        fav.style.color = on ? "" : "var(--danger)";
        break;
      case "viewer-details": toggleDetails(); break;
      case "viewer-edit": toggleEditor(); break;
      case "viewer-delete":
        toast("已移到回收站", {
          kind: "success",
          action: { label: "撤销", run: function () { toast("已还原", { kind: "info" }); } }
        });
        stepViewer(1);
        break;
      case "editor-close": case "editor-cancel": attemptCloseEditor(); break;
      case "editor-reset":
        setDirty(false); editor.values = {}; applyEditFilter();
        qa(".scale[data-adjust]").forEach(function (s) { var f = q(".scale-fill", s); if (f) f.style.width = "0%"; });
        break;
      case "editor-compare":
        editor.compare = !editor.compare;
        el.classList.toggle("active", editor.compare);
        applyEditFilter();
        break;
      case "editor-save-copy":
        setDirty(false);
        toast("已保存副本 IMG_0123_copy.jpg", { kind: "success" });
        break;
      case "editor-overwrite":
        setDirty(false);
        toast("已覆盖原文件（保留 .bak）", { kind: "success" });
        break;
      case "discard-confirm":
        q("#discard-dialog").classList.remove("is-open");
        setDirty(false); editor.values = {}; applyEditFilter();
        var panel = q(".side-panel.editor");
        if (panel) panel.classList.remove("open");
        body.dataset.pvEditing = "0";
        break;
      case "discard-keep": q("#discard-dialog").classList.remove("is-open"); break;
      case "open-shortcuts": openShortcuts(true); break;
      case "close-shortcuts": openShortcuts(false); break;
      case "toggle-motion": setMotion(body.dataset.pvMotion !== "off"); emit("motionUI"); break;
      case "toggle-material": setMaterial(body.dataset.pvMaterial === "liquid" ? "plain" : "liquid"); emit("materialUI"); break;
      case "close-settings": back(); break;
      case "close-picker": back(); break;
      case "toggle-overlay":
        q(".collapsed-workspace").classList.toggle("overlay-open");
        break;
      case "sync-retry": toast("重新触发一次同步任务", { kind: "info" }); break;
      case "toggle-overview":
        // P2-8 已落盘：chevron 只是 revealer 的镜像，开合都回到同一个状态位。
        setOverview(overviewIsOpen() ? "closed" : "open");
        break;
      case "scan-retry": setScan("scanning"); setTimeout(function () { setScan("ready"); }, 2600); break;
      case "err-retry": toast("重试解码：仍失败（文件不存在）", { kind: "error" }); break;
      case "picker-retry": body.dataset.pvDemo = ""; break;
      case "empty-trash":
        toast("清空回收站？9 项将被永久删除", { kind: "error", ms: 4200 });
        break;
      case "restore": toast("已还原 " + (selectedIds().length || 1) + " 项", { kind: "success" }); break;
      case "purge": toast("永久删除前需要确认（现状已有 Adw.AlertDialog）", { kind: "error" }); break;
      default: break;
    }
  }

  function openShortcuts(open) {
    var win = q("#shortcuts-window");
    if (!win) return;
    // P0-4 已落盘，所以窗口任何时候都能打开；只有「落盘前」演示态才模拟
    // 当时应用内没有任何快捷键入口的情况。
    if (body.dataset.pvDemo === "shortcut-blind") {
      toast("落盘前对照：应用内没有任何快捷键发现入口，F1 无反应（P0-4 已落盘，清除演示态即可打开）", { kind: "error", ms: 4200 });
      return;
    }
    win.classList.toggle("is-open", open !== false);
  }

  /* --------------------------------------------------- 键盘与指针输入 */

  function gridColumns(grid) {
    var style = getComputedStyle(grid);
    if (style.gridTemplateColumns) {
      return style.gridTemplateColumns.split(" ").filter(Boolean).length || 1;
    }
    return 1;
  }
  function moveTileFocus(tile, key) {
    var grid = tile.closest(".tile-grid");
    if (!grid) return false;
    var tiles = qa(".tile[data-media]", grid);
    var cols = gridColumns(grid);
    var i = tiles.indexOf(tile);
    var next = i;
    if (key === "ArrowLeft") next = i - 1;
    else if (key === "ArrowRight") next = i + 1;
    else if (key === "ArrowUp") next = i - cols;
    else if (key === "ArrowDown") next = i + cols;
    if (next < 0 || next >= tiles.length) return false;
    tiles[next].focus();
    // :focus-visible 对脚本 focus 的判定是浏览器启发式，评审要可复现就显式标记。
    qa(".tile.kbd-focus").forEach(function (t) { t.classList.remove("kbd-focus"); });
    tiles[next].classList.add("kbd-focus");
    tiles[next].scrollIntoView({ block: "nearest", behavior: body.dataset.pvMotion === "off" ? "auto" : "smooth" });
    return true;
  }

  function wireKeyboard() {
    document.addEventListener("keydown", function (ev) {
      if (ev.target.tagName === "INPUT" || ev.target.tagName === "TEXTAREA") {
        if (ev.key === "Escape") ev.target.blur();
        return;
      }
      var key = ev.key;
      var ctrl = ev.ctrlKey || ev.metaKey;
      var tile = document.activeElement && document.activeElement.classList &&
        document.activeElement.classList.contains("tile") ? document.activeElement : null;

      // P2-3 已落盘：胶囊本身是唯一 tab stop，左右方向键在段间循环并环绕，
      // 焦点始终留在整件控件上（与 mode_selector.rs 的 EventControllerKey 同语义）。
      var segBar = ev.target.closest ? ev.target.closest(".mode-selector") : null;
      if (segBar && (key === "ArrowLeft" || key === "ArrowRight")) {
        ev.preventDefault();
        var segs = qa(".segment", segBar);
        if (!segs.length) return;
        var current = segs.findIndex(function (seg) { return seg.classList.contains("active"); });
        if (current < 0) current = 0;
        var next = (current + (key === "ArrowRight" ? 1 : segs.length - 1)) % segs.length;
        selectSegment(segs[next]);
        return;
      }

      if (key === "F1" || (ctrl && (key === "/" || key === "?"))) {
        ev.preventDefault(); openShortcuts(); return;
      }
      if (ctrl && key === ",") { ev.preventDefault(); go("settings"); return; }
      if (ctrl && (key === "f" || key === "F")) { ev.preventDefault(); go("search"); return; }
      if (ev.altKey && key === "ArrowLeft") { ev.preventDefault(); back(); return; }
      if (key === "Escape") {
        if (q("#discard-dialog.is-open")) { q("#discard-dialog").classList.remove("is-open"); return; }
        if (q("#shortcuts-window.is-open")) { q("#shortcuts-window").classList.remove("is-open"); return; }
        if (q("#context-menu.is-open")) { q("#context-menu").classList.remove("is-open"); return; }
        if (body.dataset.pvEditing === "1" && editor.dirty) { attemptCloseEditor(); return; }
        if (body.dataset.pvMode === "run" && current !== "photos") { back(); return; }
      }
      if (ctrl && (key === "a" || key === "A")) {
        if (multi || true) { setMulti(true); selectAllInScreen(); ev.preventDefault(); }
        return;
      }
      if (key === " " && tile) {
        ev.preventDefault();
        if (!multi) setMulti(true);
        toggleTile(tile);
        return;
      }
      if (key === "Enter" && tile) {
        ev.preventDefault();
        var grid = tile.closest(".tile-grid");
        var tiles = qa(".tile[data-media]", grid);
        openViewer(tiles, tiles.indexOf(tile));
        return;
      }
      if (key === "Delete") {
        if (selectedIds().length) { runAction("batch-trash"); }
        return;
      }
      if (tile && ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].indexOf(key) >= 0) {
        if (moveTileFocus(tile, key)) ev.preventDefault();
        return;
      }
      if (body.dataset.pvScreen !== "viewer") return;
      switch (key) {
        case "ArrowLeft": ev.preventDefault(); stepViewer(-1); break;
        case "ArrowRight": ev.preventDefault(); stepViewer(1); break;
        case "+": case "=": stepZoom(1); break;
        case "-": stepZoom(-1); break;
        case "0": resetTransform(); break;
        case "r": viewer.rot = (viewer.rot + 90) % 360; applyTransform(); break;
        case "R": viewer.rot = (viewer.rot + 270) % 360; applyTransform(); break;
        case "i": case "I": toggleDetails(); break;
        case "e": case "E": toggleEditor(); break;
        case "h": case "H": runAction("viewer-fav", q('[data-act="viewer-fav"]')); break;
        case "f": case "F":
          toast("F 打开的是独立无边框顶层窗口（结构性议题，见 backlog 文末）", { kind: "info", ms: 3600 });
          break;
        default: break;
      }
    });
  }

  function wireViewerInput() {
    var stage = q(".viewer-stage");
    if (!stage) return;
    stage.addEventListener("wheel", function (ev) {
      if (!enabled("p1-8")) return;
      if (!(ev.ctrlKey || ev.metaKey)) return;   // 无 Ctrl 时不劫持普通滚动
      ev.preventDefault();
      stepZoom(ev.deltaY < 0 ? 1 : -1);
    }, { passive: false });

    var dragging = false, startX = 0, startY = 0, baseX = 0, baseY = 0;
    var media = q(".viewer-media");
    media.addEventListener("pointerdown", function (ev) {
      if (!enabled("p1-8") || viewer.scale <= 1.0001) return;   // fit 态不平移（原设计意图）
      dragging = true;
      startX = ev.clientX; startY = ev.clientY;
      baseX = viewer.panX; baseY = viewer.panY;
      media.classList.add("is-panning");
      media.setPointerCapture(ev.pointerId);
    });
    media.addEventListener("pointermove", function (ev) {
      if (!dragging) return;
      viewer.panX = baseX + (ev.clientX - startX);
      viewer.panY = baseY + (ev.clientY - startY);
      clampPan();
      applyTransform();
    });
    ["pointerup", "pointercancel"].forEach(function (evt) {
      media.addEventListener(evt, function () {
        dragging = false;
        media.classList.remove("is-panning");
      });
    });

    qa('[data-ctrl]').forEach(function (btn) {
      btn.addEventListener("click", function () {
        switch (btn.dataset.ctrl) {
          case "zoom-in": stepZoom(1); break;
          case "zoom-out": stepZoom(-1); break;
          case "reset": resetTransform(); break;
          case "rotate-left": viewer.rot = (viewer.rot + 270) % 360; applyTransform(); break;
          case "rotate-right": viewer.rot = (viewer.rot + 90) % 360; applyTransform(); break;
          case "fullscreen":
            toast("全屏预览：独立 decorated(false) 顶层窗口，Esc 回到另一个实例", { kind: "info", ms: 4000 });
            break;
          default: break;
        }
      });
    });
    qa('[data-ctrl], .zoom-level').forEach(function (el) {
      el.addEventListener("click", function (ev) { ev.stopPropagation(); });
    });
  }

  function wireEditorScales() {
    qa(".scale[data-adjust]").forEach(function (sc) {
      function setFromEvent(ev) {
        var rect = sc.getBoundingClientRect();
        var ratio = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
        sc.querySelector(".scale-fill").style.width = Math.round(ratio * 100) + "%";
        editor.values[sc.dataset.adjust] = Number(((ratio - 0.5) * 0.9).toFixed(2));
        markDirty();
      }
      var active = false;
      sc.addEventListener("pointerdown", function (ev) { active = true; setFromEvent(ev); });
      sc.addEventListener("pointermove", function (ev) { if (active) setFromEvent(ev); });
      ["pointerup", "pointerleave"].forEach(function (e) {
        sc.addEventListener(e, function () { active = false; });
      });
    });
  }

  function wireSearch() {
    var input = q("#search-input");
    if (!input) return;
    input.addEventListener("input", function () { runSearch(input.value); });
    input.addEventListener("focus", function () { go("search", { push: false }); });
  }

  function wireChromeControls() {
    var range = q("#transparency-range");
    if (range) range.addEventListener("input", function () { setTransparency(range.value); });
  }

  /* ---------------------------------------------------------- 启动 */

  function boot() {
    applyImages();
    applyModeSelectorA11y();
    applyIconLabels();
    body.dataset.pvSearch = "idle";
    body.dataset.pvGridMode = "year";
    body.dataset.multi = "0";
    // P2-8：真实应用启动时总览是收起的，chevron 因此显示「展开」。
    body.dataset.pvOvr = "closed";
    applyOverview();
    body.dataset.pvEditing = "0";
    wireNavigation();
    wireKeyboard();
    wireViewerInput();
    wireEditorScales();
    wireSearch();
    wireChromeControls();
    setTransparency(0);
    applyLocale();
    renderViewerChrome();
    updateCount();
    // 提案开关切换后需要重算依赖 token 的状态（对勾常驻、禁用态、可平移类等），
    // 否则要等到下一次选择/缩放才生效。
    on("proposalChange", function () {
      paintSelection();
      renderViewerChrome();
      renderViewer();
      applyTransform();
      applyEditFilter();
      // P1-8：切换提案或页面时同步提示的可见状态。
      revealStageHint(enabled("p1-8") && current === "viewer");
      applyModeSelectorA11y();
      applyIconLabels();
    });
    document.addEventListener("focusin", function (ev) {
      if (ev.target && !ev.target.classList.contains("tile")) {
        qa(".tile.kbd-focus").forEach(function (t) { t.classList.remove("kbd-focus"); });
      }
    });
    emit("ready");
  }

  window.PV = {
    on: on, emit: emit,
    q: q, qa: qa,
    tokens: tokens, enabled: enabled, setEnabled: setEnabled, toggle: toggle,
    go: go, back: back, setMode: setMode, setScan: setScan,
    setTransparency: setTransparency, setMotion: setMotion, setMaterial: setMaterial,
    setWidth: setWidth, setLocale: setLocale, applyLocale: applyLocale,
    setOverview: setOverview, applyOverview: applyOverview,
    setMulti: setMulti, toggleTile: toggleTile, selectedIds: selectedIds, updateCount: updateCount,
    runSearch: runSearch, setSearchLatency: setSearchLatency,
    selectAll: selectAllInScreen, simulateRebuild: simulateRebuild,
    openViewer: openViewer, stepViewer: stepViewer, stepZoom: stepZoom,
    resetTransform: resetTransform, toggleDetails: toggleDetails, toggleEditor: toggleEditor,
    attemptCloseEditor: attemptCloseEditor, openShortcuts: openShortcuts,
    revealStageHint: revealStageHint,
    applyModeSelectorA11y: applyModeSelectorA11y, applyIconLabels: applyIconLabels,
    toast: toast, runAction: runAction, contrastReport: contrastReport,
    get screen() { return current; },
    get multi() { return multi; },
    get editorDirty() { return editor.dirty; },
    setEditorDirty: markDirty,
    applyImages: applyImages
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
