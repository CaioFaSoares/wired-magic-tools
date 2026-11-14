(() => {
  "use strict";

  const $ = (id) => document.getElementById(id);

  const state = {
    interval: 1000,
    timer: null,
    polling: false,
    failures: 0,
    cores: 0,
  };

  const timeFormat = new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });

  function clamp(value, min, max) {
    return Math.min(max, Math.max(min, value));
  }

  function pct(value) {
    return Number.isFinite(value) ? clamp(value, 0, 100) : 0;
  }

  function setText(id, value) {
    const element = $(id);
    if (element) element.textContent = value == null || value === "" ? "—" : String(value);
  }

  function setMeter(id, value) {
    const element = $(id);
    if (!element) return;
    const valuePct = pct(value);
    element.style.width = `${valuePct}%`;
    element.classList.toggle("is-warn", valuePct >= 75 && valuePct < 90);
    element.classList.toggle("is-danger", valuePct >= 90);
  }

  function formatBytes(bytes) {
    if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
    const units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let value = bytes;
    let unit = 0;
    while (value >= 1024 && unit < units.length - 1) {
      value /= 1024;
      unit += 1;
    }
    return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
  }

  function formatRate(bytes) {
    return `${formatBytes(bytes)}/s`;
  }

  function formatPercent(value) {
    return `${pct(value).toFixed(1)}%`;
  }

  function formatDuration(seconds) {
    if (!Number.isFinite(seconds) || seconds <= 0) return "0s";
    const total = Math.floor(seconds);
    const days = Math.floor(total / 86400);
    const hours = Math.floor((total % 86400) / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    if (days > 0) return `${days}d ${hours}h`;
    if (hours > 0) return `${hours}h ${minutes}m`;
    if (minutes > 0) return `${minutes}m ${total % 60}s`;
    return `${total}s`;
  }

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = String(text);
    return node;
  }

  function textCell(value, className) {
    const td = document.createElement("td");
    if (className) td.className = className;
    td.textContent = value == null || value === "" ? "—" : String(value);
    return td;
  }

  function usageCell(percentValue) {
    const wrap = element("div", "usage");
    const meter = element("div", "usage-bar");
    const fill = document.createElement("span");
    const value = pct(percentValue);
    fill.style.width = `${value}%`;
    fill.classList.toggle("is-warn", value >= 75 && value < 90);
    fill.classList.toggle("is-danger", value >= 90);
    meter.append(fill);
    wrap.append(meter, element("em", null, `${value.toFixed(0)}%`));
    return wrap;
  }

  function emptyRow(tbody, text, columns) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.className = "empty-cell";
    td.colSpan = columns;
    td.textContent = text;
    tr.append(td);
    tbody.replaceChildren(tr);
  }

  function renderCores(perCore) {
    const container = $("core-bars");
    const values = Array.isArray(perCore) ? perCore : [];
    if (container.children.length !== values.length) {
      container.replaceChildren();
      for (let index = 0; index < values.length; index += 1) {
        const core = element("div", "core");
        const label = element("div", "core-label");
        label.append(element("span", null, `#${index}`), element("strong", null, "0%"));
        const meter = element("div", "core-meter");
        const fill = document.createElement("span");
        meter.append(fill);
        core.append(label, meter);
        container.append(core);
      }
      state.cores = values.length;
    }

    values.forEach((value, index) => {
      const core = container.children[index];
      if (!core) return;
      const valuePct = pct(value);
      const strong = core.querySelector("strong");
      const fill = core.querySelector(".core-meter span");
      if (strong) strong.textContent = `${valuePct.toFixed(0)}%`;
      if (fill) {
        fill.style.width = `${valuePct}%`;
        fill.classList.toggle("is-warn", valuePct >= 75 && valuePct < 90);
        fill.classList.toggle("is-danger", valuePct >= 90);
      }
    });
  }

  function renderSnapshot(snapshot) {
    if (!snapshot || !snapshot.host) return;

    const host = snapshot.host;
    setText("host-name", host.hostname);
    setText("os-name", host.long_os_version || host.os_version || host.name);
    setText("kernel", host.kernel);
    setText("arch", host.arch);
    setText("host-chip", host.hostname);
    setText("uptime-chip", `↑ ${formatDuration(host.uptime_secs)}`);

    const cpu = snapshot.cpu || {};
    setText("cpu-usage", formatPercent(cpu.usage_percent));
    setText(
      "cpu-brand",
      `${cpu.brand || "CPU"} · ${cpu.cores || 0} núcleos${
        cpu.physical_cores ? ` (${cpu.physical_cores} físicos)` : ""
      }${cpu.frequency_mhz ? ` · ${cpu.frequency_mhz} MHz` : ""}`,
    );
    setText("cores-chip", `${cpu.cores || 0} núcleos`);
    setMeter("cpu-bar", cpu.usage_percent);
    renderCores(cpu.per_core);

    const memory = snapshot.memory || {};
    const memoryPct =
      memory.total_bytes > 0 ? (memory.used_bytes / memory.total_bytes) * 100 : 0;
    setText("mem-usage", formatPercent(memoryPct));
    setText(
      "mem-meta",
      `${formatBytes(memory.used_bytes)} de ${formatBytes(memory.total_bytes)} · ${formatBytes(
        memory.available_bytes,
      )} livres`,
    );
    setMeter("mem-bar", memoryPct);

    const load = snapshot.load || {};
    setText("load-1", Number.isFinite(load.one) ? load.one.toFixed(2) : "—");
    setText(
      "load-meta",
      `${Number.isFinite(load.five) ? load.five.toFixed(2) : "—"} · ${
        Number.isFinite(load.fifteen) ? load.fifteen.toFixed(2) : "—"
      }`,
    );

    const wm = snapshot.wm;
    if (wm) {
      setText("wm-cpu", `${wm.cpu_percent.toFixed(1)}%`);
      setText("wm-meta", `${wm.name || "wm"} · pid ${wm.pid} · ${wm.state}`);
      setText("wm-pid", `RSS ${formatBytes(wm.memory_bytes)} · ${formatDuration(wm.run_time_secs)}`);
      setText("wm-state", wm.state || "—");
      setText("wm-pid-detail", wm.pid);
      setText("wm-ppid", wm.parent ?? "—");
      setText("wm-cpu-detail", `${wm.cpu_percent.toFixed(1)}%`);
      setText("wm-rss", formatBytes(wm.memory_bytes));
      setText("wm-virt", formatBytes(wm.virtual_memory_bytes));
      setText("wm-uptime", formatDuration(wm.run_time_secs));
      setText("wm-cmd", wm.cmd || "(linha de comando indisponível)");
    } else {
      setText("wm-cpu", "—");
      setText("wm-meta", "processo wm não encontrado");
      setText("wm-pid", "—");
      setText("wm-state", "ausente");
      setText("wm-pid-detail", "—");
      setText("wm-ppid", "—");
      setText("wm-cpu-detail", "—");
      setText("wm-rss", "—");
      setText("wm-virt", "—");
      setText("wm-uptime", "—");
      setText("wm-cmd", "—");
    }

    const disksBody = $("disks-body");
    const disks = Array.isArray(snapshot.disks) ? snapshot.disks : [];
    setText("disks-chip", `${disks.length} volume(s)`);
    if (disks.length === 0) {
      emptyRow(disksBody, "Nenhum disco reportado.", 3);
    } else {
      disksBody.replaceChildren(
        ...disks.map((disk) => {
          const tr = document.createElement("tr");
          const mount = textCell(`${disk.mount}${disk.removable ? " · removível" : ""}`);
          mount.append(element("span", "sub", disk.file_system || disk.name));
          const usage = document.createElement("td");
          usage.append(usageCell(disk.usage_percent));
          tr.append(mount, usage, textCell(formatBytes(disk.available_bytes), "num"));
          return tr;
        }),
      );
    }

    const netBody = $("net-body");
    const networks = Array.isArray(snapshot.networks) ? snapshot.networks : [];
    setText("net-chip", `${networks.length} interface(s)`);
    if (networks.length === 0) {
      emptyRow(netBody, "Nenhuma interface com tráfego.", 4);
    } else {
      netBody.replaceChildren(
        ...networks.map((net) => {
          const tr = document.createElement("tr");
          tr.append(
            textCell(net.name),
            textCell(formatRate(net.rx_per_sec), "num"),
            textCell(formatRate(net.tx_per_sec), "num"),
            textCell(formatBytes((net.rx_total_bytes || 0) + (net.tx_total_bytes || 0)), "num"),
          );
          return tr;
        }),
      );
    }

    const processBody = $("proc-body");
    const processes = Array.isArray(snapshot.processes) ? snapshot.processes : [];
    setText("proc-chip", `${processes.length} processo(s)`);
    if (processes.length === 0) {
      emptyRow(processBody, "Nenhum processo reportado.", 5);
    } else {
      processBody.replaceChildren(
        ...processes.map((process) => {
          const tr = document.createElement("tr");
          tr.append(
            textCell(process.pid, "num"),
            textCell(process.name),
            textCell(`${process.cpu_percent.toFixed(1)}%`, "num"),
            textCell(formatBytes(process.memory_bytes), "num"),
            textCell(process.state),
          );
          return tr;
        }),
      );
    }

    const sampled = new Date(snapshot.generated_at);
    setText(
      "timestamp",
      Number.isNaN(sampled.getTime()) ? "amostra —" : `amostra ${timeFormat.format(sampled)}`,
    );
  }

  function applyInfo(info) {
    if (!info) return;
    if (typeof info.title === "string" && info.title.trim()) {
      const title = info.title.trim();
      $("panel-title").textContent = title;
      document.title = `${title} · wired-magic`;
    }
    setText("version", `wm ${info.version || "—"}`);
    if (Number.isFinite(info.interval_secs) && info.interval_secs > 0) {
      state.interval = info.interval_secs * 1000;
    }
    if (typeof info.node === "string" && info.node.trim()) {
      const chip = $("node-chip");
      chip.textContent = `nó: ${info.node.trim()}`;
      chip.hidden = false;
    }
  }

  function setStatus(text, tone) {
    const hint = $("hint");
    hint.textContent = text;
    hint.classList.toggle("is-online", tone === "online");
    hint.classList.toggle("is-offline", tone === "offline");
  }

  async function getJson(url) {
    const response = await fetch(url, { cache: "no-store", headers: { Accept: "application/json" } });
    if (!response.ok) {
      let detail = `HTTP ${response.status}`;
      try {
        const body = await response.json();
        if (body && typeof body.error === "string") detail = body.error;
      } catch {
        /* keep status */
      }
      throw new Error(detail);
    }
    return response.json();
  }

  async function poll() {
    if (state.polling) return;
    state.polling = true;
    try {
      const snapshot = await getJson("/api/snapshot");
      renderSnapshot(snapshot);
      state.failures = 0;
      setStatus(`ao vivo · amostra a cada ${state.interval / 1000}s`, "online");
    } catch (error) {
      state.failures += 1;
      setStatus(`sem conexão com o painel (${error.message}) · tentando de novo…`, "offline");
    } finally {
      state.polling = false;
      const delay = state.failures > 0 ? Math.min(15000, state.interval * state.failures) : state.interval;
      window.clearTimeout(state.timer);
      state.timer = window.setTimeout(poll, delay);
    }
  }

  async function boot() {
    try {
      const info = await getJson("/api/info");
      applyInfo(info);
      await poll();
    } catch (error) {
      setStatus(`não foi possível carregar o painel: ${error.message}`, "offline");
      window.setTimeout(poll, 3000);
    }
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) poll();
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot, { once: true });
  } else {
    boot();
  }
})();
