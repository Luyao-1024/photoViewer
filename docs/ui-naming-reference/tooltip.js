const tooltip = document.getElementById("tooltip");

function contentFor(target) {
  if (!target) return "";
  const name = target.dataset.name || target.dataset.ui || "";
  const impl = target.dataset.impl || "";
  const props = target.dataset.props || "";
  const source = target.dataset.source || "";
  return [
    `<strong>${escapeHtml(name)}</strong>`,
    impl ? `<div class="impl">${escapeHtml(impl)}</div>` : "",
    props ? `<div class="props">${escapeHtml(props)}</div>` : "",
    source ? `<div class="source">${escapeHtml(source)}</div>` : ""
  ].join("");
}

function escapeHtml(value) {
  return value.replace(/[&<>"']/g, char => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;"
  }[char]));
}

function moveTooltip(x, y) {
  const margin = 12;
  tooltip.style.left = "0px";
  tooltip.style.top = "0px";
  const rect = tooltip.getBoundingClientRect();
  let left = x + 14;
  let top = y + 14;
  if (left + rect.width + margin > window.innerWidth) left = x - rect.width - 14;
  if (top + rect.height + margin > window.innerHeight) top = y - rect.height - 14;
  tooltip.style.left = `${Math.max(margin, left)}px`;
  tooltip.style.top = `${Math.max(margin, top)}px`;
}

function showTooltip(target, x, y) {
  tooltip.innerHTML = contentFor(target);
  tooltip.classList.add("visible");
  tooltip.setAttribute("aria-hidden", "false");
  moveTooltip(x, y);
}

function hideTooltip() {
  tooltip.classList.remove("visible");
  tooltip.setAttribute("aria-hidden", "true");
}

document.addEventListener("pointermove", event => {
  const target = event.target.closest("[data-ui]");
  if (!target) {
    hideTooltip();
    return;
  }
  showTooltip(target, event.clientX, event.clientY);
});

document.addEventListener("focusin", event => {
  const target = event.target.closest("[data-ui]");
  if (!target) return;
  const rect = target.getBoundingClientRect();
  showTooltip(target, rect.left + rect.width / 2, rect.top + rect.height / 2);
});

document.addEventListener("focusout", event => {
  if (!event.relatedTarget || !event.relatedTarget.closest("[data-ui]")) {
    hideTooltip();
  }
});

document.addEventListener("pointerleave", hideTooltip);
