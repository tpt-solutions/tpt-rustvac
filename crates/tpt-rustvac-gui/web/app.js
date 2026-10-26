// The page. It draws the state the app reports and sends the user's choices back as commands.
// The app holds all the state; this file only shows it and asks for changes.
(() => {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const els = {
    modes: document.querySelectorAll("[data-mode]"),
    rootForm: $("root-form"),
    root: $("root"),
    roots: $("roots"),
    search: $("search"),
    minAge: $("min-age"),
    minSize: $("min-size"),
    hideArchived: $("hide-archived"),
    sort: $("sort"),
    notice: $("notice"),
    tree: $("tree"),
    empty: $("empty"),
    detailsBody: $("details-body"),
    compress: $("act-compress"),
    trash: $("act-trash"),
    reveal: $("act-reveal"),
    copy: $("act-copy"),
    status: $("status"),
    targets: $("stat-targets"),
    reclaim: $("stat-reclaim"),
    last: $("stat-last"),
    failures: $("failures"),
    failureCount: $("failure-count"),
    failureList: $("failure-list"),
    menu: $("menu"),
    confirm: $("confirm"),
    confirmTitle: $("confirm-title"),
    confirmSummary: $("confirm-summary"),
    confirmList: $("confirm-list"),
    confirmNote: $("confirm-note"),
    confirmCancel: $("confirm-cancel"),
    confirmOk: $("confirm-ok"),
  };

  // The last state the app sent. Null until the first answer arrives.
  let state = null;
  // The action waiting for the user to confirm it in the dialog.
  let pending = null;
  let searchTimer = 0;
  // The message from the last action or error, shown in the notice bar.
  let problem = "";
  let problemIsError = true;

  // ---- talking to the app -------------------------------------------------------------

  function call(command, args = {}) {
    if (!window.tpt) {
      return Promise.reject({ message: "Open this page in the app window." });
    }
    return window.tpt.invoke(command, args);
  }

  function describe(error) {
    if (error && error.message) {
      return error.code ? `${error.code}: ${error.message}` : error.message;
    }
    return String(error);
  }

  // Sends a change, then redraws from the app's answer.
  async function send(command, args) {
    try {
      const reply = await call(command, args);
      notice("");
      await pull();
      return reply;
    } catch (error) {
      notice(describe(error));
      return null;
    }
  }

  // Reads the state. The app advances its scan and work threads on each read.
  async function pull() {
    try {
      state = await call("state.get");
      render();
    } catch (error) {
      notice(describe(error));
    }
  }

  async function loop() {
    await pull();
    const busy = state && (state.scanning || state.working);
    setTimeout(loop, busy ? 250 : 1000);
  }

  // ---- formatting ---------------------------------------------------------------------

  function human(bytes) {
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    return unit === 0 ? `${bytes} B` : `${value.toFixed(1)} ${units[unit]}`;
  }

  function ageText(days) {
    if (days === null || days === undefined) return "unknown";
    if (days === 0) return "today";
    return `${days} d`;
  }

  // Under 7 days dim, 7 to 30 normal, 31 to 90 amber, over 90 red.
  function ageClass(days) {
    if (days === null || days === undefined) return "";
    if (days < 7) return "age-dim";
    if (days > 90) return "age-stale";
    if (days > 30) return "age-old";
    return "";
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  // Sets the message from the last action. It stays until the next action replaces it.
  // Errors are drawn in the danger colour; confirmations are not.
  function notice(text, isError = true) {
    problem = text || "";
    problemIsError = isError;
    showNotice();
  }

  // Shows the last action's message, with the app's warnings after it.
  function showNotice() {
    const parts = [problem];
    if (state && state.configError) parts.push(`Config: ${state.configError}`);
    if (state && state.rowsTruncated) {
      parts.push("Showing the first rows only. Narrow the search to see the rest.");
    }
    const text = parts.filter(Boolean).join(" · ");
    els.notice.hidden = !text;
    els.notice.textContent = text;
    els.notice.classList.toggle("error", problemIsError || !!(state && state.configError));
  }

  // ---- drawing ------------------------------------------------------------------------

  function render() {
    if (!state) return;
    renderToolbar();
    renderTree(state.rows);
    renderDetails();
    renderStatus();
    showNotice();
  }

  function renderToolbar() {
    for (const button of els.modes) {
      button.setAttribute("aria-pressed", String(button.dataset.mode === state.mode));
    }
    if (document.activeElement !== els.root) els.root.value = state.root;
    els.roots.replaceChildren(
      ...state.roots.map((path) => el("option", "", path)),
    );
    if (document.activeElement !== els.search) els.search.value = state.query;
    if (document.activeElement !== els.minAge) els.minAge.value = state.minAgeDays;
    if (document.activeElement !== els.minSize) els.minSize.value = state.minSizeMb;
    els.hideArchived.checked = state.hideArchived;
    els.sort.textContent = state.sortDesc ? "Size ↓" : "Size ↑";
  }

  function renderTree(rows) {
    const max = rows.reduce((top, row) => Math.max(top, row.size), 1);
    const fragment = document.createDocumentFragment();
    for (const row of rows) fragment.append(rowElement(row, max));
    els.tree.replaceChildren(fragment);
    els.empty.hidden = rows.length > 0;
    if (rows.length === 0) {
      els.empty.textContent = state.scanning
        ? "Scanning…"
        : "Nothing matches. Try clearing the search or the filters.";
    }
  }

  function rowElement(row, max) {
    const line = el("div", "row");
    line.dataset.id = String(row.id);
    line.setAttribute("role", "treeitem");
    line.setAttribute("aria-level", String(row.depth + 1));
    line.setAttribute("aria-selected", String(row.selected));
    if (row.selected) line.classList.add("selected");
    if (row.state === "archived") line.classList.add("archived");
    if (row.state === "failed") line.classList.add("failed");

    const caret = el("button", "caret", row.hasChildren ? (row.expanded ? "▾" : "▸") : "");
    caret.type = "button";
    caret.dataset.act = "expand";
    caret.disabled = !row.hasChildren;
    caret.setAttribute("aria-label", row.expanded ? "Collapse" : "Expand");

    const check = el("input");
    check.type = "checkbox";
    check.checked = row.checked;
    check.dataset.act = "check";
    check.disabled = row.kind === "root";
    check.setAttribute("aria-label", `Check ${row.name}`);

    const name = el("div", "name", row.name);
    name.style.paddingLeft = `${row.depth * 16}px`;
    name.title = row.path;
    if (state.mode === "mixed" && row.target) name.append(el("span", "badge", "rust"));
    if (row.state === "archived") name.append(el("span", "badge", "archived"));
    if (row.state === "busy") name.append(el("span", "badge", "busy"));

    const size = el("div", "size", human(row.size));
    const bar = el("div", "bar");
    const fill = el("i");
    fill.style.width = `${((row.size / max) * 100).toFixed(1)}%`;
    bar.append(fill);

    const age = el("div", `num ${ageClass(row.ageDays)}`, ageText(row.ageDays));

    line.append(caret, check, name, size, bar, age);
    return line;
  }

  function renderDetails() {
    const node = state.selected;
    const canAct = !!node && !state.working;
    els.compress.disabled = !canAct || !node.target;
    els.trash.disabled = !canAct;
    els.reveal.disabled = !node;
    els.copy.disabled = !node;

    if (!node) {
      els.detailsBody.replaceChildren(
        el("p", "muted", "Select a folder to see its details."),
      );
      return;
    }
    const list = el("dl");
    const fields = [
      ["Name", node.name],
      ["Path", node.path],
      ["Kind", node.kind],
      ["State", node.state],
      ["Size", `${human(node.size)} (${node.size.toLocaleString()} bytes)`],
      ["Files", node.files.toLocaleString()],
      ["Newest file", ageText(node.ageDays)],
    ];
    for (const [label, value] of fields) {
      list.append(el("dt", "", label), el("dd", "", value));
    }
    els.detailsBody.replaceChildren(list);
  }

  function renderStatus() {
    let text = state.status;
    if (!text) {
      text = state.scanning
        ? `Scanning ${state.progress.done}/${state.progress.total}`
        : "Ready";
    }
    els.status.textContent = text;
    els.targets.textContent = `${state.targets} targets`;
    els.reclaim.textContent = `Reclaimable ${human(state.reclaimable)}`;
    els.last.textContent =
      state.lastScan === null
        ? "No scan yet"
        : `Last scan ${new Date(state.lastScan).toLocaleTimeString()}`;

    els.failures.hidden = state.failureCount === 0;
    els.failureCount.textContent = `${state.failureCount} failed`;
    els.failureList.replaceChildren(
      ...state.failures.map((line) => el("li", "", line)),
    );
  }

  // ---- selection and checks ------------------------------------------------------------

  function selectedId() {
    return state && state.selected ? state.selected.id : null;
  }

  // The rows the actions apply to: every checked row, or else the selected one.
  function actionIds() {
    if (!state) return [];
    if (state.checked.length > 0) return state.checked;
    const id = selectedId();
    return id === null ? [] : [id];
  }

  // ---- actions ------------------------------------------------------------------------

  async function askAction(kind) {
    const ids = actionIds();
    if (ids.length === 0) {
      notice("Select a folder, or check some, first.");
      return;
    }
    const prepared = await call("action.prepare", { action: kind, ids }).catch((error) => {
      notice(describe(error));
      return null;
    });
    if (!prepared) return;
    if (prepared.items.length === 0) {
      notice(prepared.note || "Nothing to do.");
      return;
    }

    pending = { action: kind, ids };
    const verb = kind === "compress" ? "Compress" : "Move to Trash";
    const count = prepared.items.length;
    els.confirmTitle.textContent = `${verb} ${count} folder${count === 1 ? "" : "s"}?`;
    els.confirmSummary.textContent =
      kind === "compress"
        ? `Each folder is archived and checked, then its original target folder goes to the Trash. About ${human(prepared.bytes)} is involved.`
        : `Each folder goes to the Trash. About ${human(prepared.bytes)} is involved.`;
    const shown = prepared.items.slice(0, 200).map(
      (item) => el("li", "", `${item.path}  (${human(item.bytes)})`),
    );
    if (count > shown.length) {
      shown.push(el("li", "muted", `…and ${count - shown.length} more`));
    }
    els.confirmList.replaceChildren(...shown);
    els.confirmNote.hidden = !prepared.note;
    els.confirmNote.textContent = prepared.note || "";
    els.confirmOk.textContent = verb;
    els.confirm.showModal();
    els.confirmOk.focus();
  }

  async function runPending() {
    const action = pending;
    pending = null;
    els.confirm.close();
    if (!action) return;
    const reply = await send("action.run", action);
    if (reply && reply.note) notice(reply.note);
  }

  async function copyPath(path) {
    try {
      await navigator.clipboard.writeText(path);
    } catch (_) {
      const field = el("textarea");
      field.value = path;
      document.body.append(field);
      field.select();
      document.execCommand("copy");
      field.remove();
    }
    notice(`Copied ${path}`, false);
  }

  function selectedNode() {
    return state ? state.selected : null;
  }

  // ---- menus and keys -----------------------------------------------------------------

  function showMenu(x, y, row) {
    const compress = els.menu.querySelector('[data-action="compress"]');
    compress.hidden = !row.target;
    els.menu.style.left = `${x}px`;
    els.menu.style.top = `${y}px`;
    els.menu.hidden = false;
  }

  function hideMenu() {
    els.menu.hidden = true;
  }

  function moveSelection(step) {
    if (!state || state.rows.length === 0) return;
    const index = state.rows.findIndex((row) => row.selected);
    const next = Math.min(state.rows.length - 1, Math.max(0, index + step));
    send("node.select", { id: state.rows[next].id });
  }

  function isChecked(id) {
    return state.checked.includes(id);
  }

  // ---- wiring -------------------------------------------------------------------------

  for (const button of els.modes) {
    button.addEventListener("click", () => {
      send("scan.start", { mode: button.dataset.mode, root: state.root });
    });
  }

  $("browse").addEventListener("click", async () => {
    const picked = await call("dialog.pick_folder", { start: state.root }).catch((error) => {
      notice(describe(error));
      return null;
    });
    if (picked) send("scan.start", { mode: state.mode, root: picked });
  });

  els.rootForm.addEventListener("submit", (event) => {
    event.preventDefault();
    send("scan.start", { mode: state.mode, root: els.root.value.trim() });
  });

  els.search.addEventListener("input", () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => send("view.set", { query: els.search.value }), 150);
  });

  els.minAge.addEventListener("change", () => {
    send("view.set", { minAgeDays: Math.max(0, Number(els.minAge.value) || 0) });
  });

  els.minSize.addEventListener("change", () => {
    send("view.set", { minSizeMb: Math.max(0, Number(els.minSize.value) || 0) });
  });

  els.hideArchived.addEventListener("change", () => {
    send("view.set", { hideArchived: els.hideArchived.checked });
  });

  els.sort.addEventListener("click", () => {
    send("view.set", { sortDesc: !state.sortDesc });
  });

  els.tree.addEventListener("click", (event) => {
    const row = event.target.closest(".row");
    if (!row) return;
    const id = Number(row.dataset.id);
    if (event.target.dataset.act === "expand") {
      send("node.expand", { id });
    } else if (event.target.dataset.act !== "check") {
      send("node.select", { id });
    }
  });

  els.tree.addEventListener("change", (event) => {
    if (event.target.dataset.act !== "check") return;
    const id = Number(event.target.closest(".row").dataset.id);
    send("node.check", { id, on: event.target.checked });
  });

  els.tree.addEventListener("contextmenu", async (event) => {
    const row = event.target.closest(".row");
    if (!row) return;
    event.preventDefault();
    const id = Number(row.dataset.id);
    if (selectedId() !== id) await send("node.select", { id });
    const node = selectedNode();
    if (node) showMenu(event.clientX, event.clientY, node);
  });

  els.menu.addEventListener("click", (event) => {
    const item = event.target.closest("[data-action]");
    if (!item) return;
    hideMenu();
    runMenuAction(item.dataset.action);
  });

  function runMenuAction(action) {
    const node = selectedNode();
    if (action === "compress" || action === "trash") askAction(action);
    else if (action === "reveal" && node) send("path.reveal", { path: node.path });
    else if (action === "copy" && node) copyPath(node.path);
  }

  document.addEventListener("click", (event) => {
    if (!els.menu.contains(event.target)) hideMenu();
  });

  els.compress.addEventListener("click", () => askAction("compress"));
  els.trash.addEventListener("click", () => askAction("trash"));
  els.reveal.addEventListener("click", () => runMenuAction("reveal"));
  els.copy.addEventListener("click", () => runMenuAction("copy"));

  els.confirmOk.addEventListener("click", runPending);
  els.confirmCancel.addEventListener("click", () => {
    pending = null;
    els.confirm.close();
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") hideMenu();
    if (els.confirm.open) return;
    if (event.target.matches("input, textarea")) {
      if (event.key === "Escape") event.target.blur();
      return;
    }
    const id = selectedId();
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
      event.preventDefault();
      send("node.check_all", { on: true });
    } else if (event.key === "/") {
      event.preventDefault();
      els.search.focus();
    } else if (event.key === "s") {
      send("view.set", { sortDesc: !state.sortDesc });
    } else if (event.key === " " && id !== null) {
      event.preventDefault();
      send("node.check", { id, on: !isChecked(id) });
    } else if (event.key === "Enter" && id !== null) {
      send("node.expand", { id });
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      moveSelection(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection(-1);
    }
  });

  loop();
})();
