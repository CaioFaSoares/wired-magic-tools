(() => {
  "use strict";

  const els = {
    form: document.getElementById("composer-form"),
    author: document.getElementById("author"),
    text: document.getElementById("text"),
    send: document.getElementById("send"),
    hint: document.getElementById("hint"),
    list: document.getElementById("messages"),
    empty: document.getElementById("empty"),
    count: document.getElementById("count"),
    title: document.getElementById("room-title"),
    subtitle: document.getElementById("room-subtitle"),
    node: document.getElementById("node-chip"),
  };

  const state = {
    cursor: 0,
    epoch: null,
    seen: new Set(),
    polling: false,
    timer: null,
    failures: 0,
    sending: false,
    maxText: 2000,
    title: "Chat público",
  };

  const timeFormat = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });

  function initials(name) {
    const parts = String(name || "")
      .trim()
      .split(/\s+/)
      .filter(Boolean);
    if (parts.length === 0) return "?";
    const first = parts[0].charAt(0);
    const last = parts.length > 1 ? parts[parts.length - 1].charAt(0) : "";
    return (first + last).toUpperCase();
  }

  function avatarClass(name) {
    const value = String(name || "");
    let hash = 0;
    for (let i = 0; i < value.length; i += 1) {
      hash = (hash * 31 + value.charCodeAt(i)) >>> 0;
    }
    return `av-${hash % 8}`;
  }

  function timeLabel(iso) {
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return "";
    return timeFormat.format(date);
  }

  function pluralMessages(count) {
    return count === 1 ? "1 mensagem" : `${count} mensagens`;
  }

  function isNearBottom() {
    const gap = els.list.scrollHeight - els.list.scrollTop - els.list.clientHeight;
    return gap < 96;
  }

  function scrollToBottom() {
    els.list.scrollTop = els.list.scrollHeight;
  }

  function setStatus(message, tone) {
    els.hint.textContent = message;
    els.hint.classList.toggle("is-online", tone === "online");
    els.hint.classList.toggle("is-offline", tone === "offline");
  }

  function refreshCount() {
    els.count.textContent = pluralMessages(state.seen.size);
    els.empty.hidden = state.seen.size > 0;
  }

  function makeMessage(message) {
    const item = document.createElement("li");
    item.className = "message";
    item.dataset.id = String(message.id);

    const avatar = document.createElement("span");
    avatar.className = `avatar ${avatarClass(message.author)}`;
    avatar.setAttribute("aria-hidden", "true");
    avatar.textContent = initials(message.author);

    const body = document.createElement("div");
    body.className = "message-body";

    const head = document.createElement("div");
    head.className = "message-head";

    const author = document.createElement("span");
    author.className = "message-author";
    author.textContent = message.author;

    const time = document.createElement("time");
    time.className = "message-time";
    time.dateTime = message.at;
    time.textContent = timeLabel(message.at);

    const text = document.createElement("p");
    text.className = "message-text";
    text.textContent = message.text;

    head.append(author, time);
    body.append(head, text);
    item.append(avatar, body);
    return item;
  }

  function appendMessages(messages, forceScroll) {
    const nearBottom = forceScroll || isNearBottom();
    const fragment = document.createDocumentFragment();
    let added = 0;

    for (const message of messages) {
      if (!message || !Number.isFinite(message.id) || state.seen.has(message.id)) continue;
      state.seen.add(message.id);
      state.cursor = Math.max(state.cursor, message.id);
      fragment.append(makeMessage(message));
      added += 1;
    }

    if (added === 0) return false;
    els.list.append(fragment);
    refreshCount();
    if (nearBottom) scrollToBottom();
    return true;
  }

  function clearMessages() {
    state.cursor = 0;
    state.seen.clear();
    els.list.replaceChildren();
    refreshCount();
  }

  async function getJson(url, options) {
    const response = await fetch(url, {
      cache: "no-store",
      ...options,
      headers: {
        Accept: "application/json",
        ...(options && options.headers ? options.headers : {}),
      },
    });

    if (!response.ok) {
      let detail = `HTTP ${response.status}`;
      try {
        const body = await response.json();
        if (body && typeof body.error === "string") detail = body.error;
      } catch {
        /* keep the status text */
      }
      const error = new Error(detail);
      error.status = response.status;
      throw error;
    }

    return response.json();
  }

  function applyInfo(info) {
    if (!info) return;
    state.epoch = info.epoch || null;
    state.maxText = Number.isFinite(info.max_text) ? info.max_text : 2000;
    els.text.maxLength = state.maxText;
    els.text.placeholder = `Escreva algo para a sala… (até ${state.maxText} caracteres)`;

    if (typeof info.title === "string" && info.title.trim()) {
      state.title = info.title.trim();
      els.title.textContent = state.title;
      document.title = `${state.title} · wired-magic`;
    }

    if (typeof info.node === "string" && info.node.trim()) {
      els.node.textContent = `nó: ${info.node.trim()}`;
      els.node.hidden = false;
    } else {
      els.node.hidden = true;
    }

    if (typeof info.subtitle === "string" && info.subtitle.trim()) {
      els.subtitle.textContent = info.subtitle.trim();
    }
  }

  async function loadHistory(after) {
    const data = await getJson(`/api/messages?after=${encodeURIComponent(after)}`);
    if (data.epoch && state.epoch && data.epoch !== state.epoch) {
      state.epoch = data.epoch;
      clearMessages();
      if (after !== 0) return loadHistory(0);
      return;
    }
    if (data.epoch && !state.epoch) state.epoch = data.epoch;
    if (Number.isFinite(data.cursor)) state.cursor = Math.max(state.cursor, data.cursor);
    appendMessages(Array.isArray(data.messages) ? data.messages : [], after === 0);
  }

  async function poll() {
    if (state.polling) return;
    state.polling = true;
    try {
      const before = state.epoch;
      const data = await getJson(`/api/messages?after=${encodeURIComponent(state.cursor)}`);
      if (data.epoch && before && data.epoch !== before) {
        state.epoch = data.epoch;
        clearMessages();
        await loadHistory(0);
      } else {
        if (data.epoch && !state.epoch) state.epoch = data.epoch;
        appendMessages(Array.isArray(data.messages) ? data.messages : []);
      }
      state.failures = 0;
      setStatus(
        `sala conectada · atualiza a cada 2 s · histórico em memória`,
        "online",
      );
    } catch (error) {
      state.failures += 1;
      setStatus(
        `sem conexão com a sala (${error.message}) · tentando de novo…`,
        "offline",
      );
    } finally {
      state.polling = false;
      const delay = state.failures > 0 ? Math.min(15000, 2000 * state.failures) : 2000;
      window.clearTimeout(state.timer);
      state.timer = window.setTimeout(poll, delay);
    }
  }

  function autoGrow() {
    els.text.style.height = "auto";
    els.text.style.height = `${Math.min(els.text.scrollHeight, 132)}px`;
  }

  function updateAuthor() {
    const name = els.author.value.trim();
    try {
      if (name) window.localStorage.setItem("wm.chat.author", name);
      else window.localStorage.removeItem("wm.chat.author");
    } catch {
      /* private mode: keep the field only */
    }
  }

  async function submit(event) {
    event.preventDefault();
    if (state.sending) return;

    const text = els.text.value.trim();
    if (!text) {
      setStatus("Escreva uma mensagem antes de enviar.", "offline");
      els.text.focus();
      return;
    }

    const author = els.author.value.trim() || "anônimo";
    state.sending = true;
    els.send.disabled = true;

    try {
      const data = await getJson("/api/messages", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ author, text }),
      });
      if (data.epoch) state.epoch = data.epoch;
      if (data.message) {
        appendMessages([data.message], true);
        state.cursor = Math.max(state.cursor, data.message.id);
      }
      els.text.value = "";
      autoGrow();
      updateAuthor();
      state.failures = 0;
      setStatus("mensagem enviada · sala conectada", "online");
      els.text.focus();
    } catch (error) {
      const tone = error.status === 400 ? "offline" : "offline";
      setStatus(`não foi possível enviar: ${error.message}`, tone);
    } finally {
      state.sending = false;
      els.send.disabled = false;
    }
  }

  async function boot() {
    try {
      const stored = window.localStorage.getItem("wm.chat.author");
      if (stored && !els.author.value) els.author.value = stored.slice(0, 32);
    } catch {
      /* ignore */
    }

    els.author.addEventListener("input", updateAuthor);
    els.text.addEventListener("input", autoGrow);
    els.text.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        els.form.requestSubmit();
      }
    });
    els.form.addEventListener("submit", submit);
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) poll();
    });

    try {
      const info = await getJson("/api/info");
      applyInfo(info);
      await loadHistory(0);
      setStatus("sala conectada · atualiza a cada 2 s · histórico em memória", "online");
    } catch (error) {
      setStatus(`não foi possível carregar a sala: ${error.message}`, "offline");
    } finally {
      window.setTimeout(poll, 2000);
    }
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot, { once: true });
  } else {
    boot();
  }
})();
