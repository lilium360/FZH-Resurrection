/** Shows `data-tip` texts in one element fixed to the window, kept inside its edges. */
export function initTips(): void {
  const tip = document.createElement("div");
  tip.className = "tip";
  tip.setAttribute("role", "tooltip");
  tip.hidden = true;
  document.body.append(tip);
  let owner: HTMLElement | null = null;

  const hide = () => {
    owner = null;
    tip.hidden = true;
  };
  const show = (el: HTMLElement) => {
    const text = el.dataset.tip;
    if (!text) return hide();
    owner = el;
    tip.textContent = text;
    tip.hidden = false;
    const r = el.getBoundingClientRect();
    const w = tip.offsetWidth;
    const h = tip.offsetHeight;
    const margin = 6;
    const below = el.dataset.tipPos?.startsWith("below") || r.top - h - 8 < margin;
    const left = Math.min(Math.max(r.left + r.width / 2 - w / 2, margin), window.innerWidth - w - margin);
    tip.style.left = `${Math.max(left, margin)}px`;
    tip.style.top = `${below ? r.bottom + 6 : r.top - h - 8}px`;
  };
  const target = (e: Event) => (e.target instanceof Element ? e.target.closest<HTMLElement>("[data-tip]") : null);

  document.addEventListener("mouseover", (e) => {
    const el = target(e);
    if (el !== owner) el ? show(el) : hide();
  });
  document.addEventListener("focusin", (e) => {
    const el = target(e);
    if (el?.matches(":focus-visible")) show(el);
  });
  document.addEventListener("focusout", hide);
  document.addEventListener("mousedown", hide);
  document.addEventListener("scroll", hide, true);
  new MutationObserver(() => {
    if (owner && !owner.isConnected) hide();
  }).observe(document.body, { childList: true, subtree: true });
}
