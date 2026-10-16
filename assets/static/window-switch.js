(() => {
  const address = () => location.pathname + location.search;
  let shown = address();
  let turn = 0;
  let pending;

  async function show(url, push = false) {
    const mine = ++turn;
    pending?.abort();
    pending = new AbortController();
    const main = document.getElementById("main");
    main?.setAttribute("aria-busy", "true");
    try {
      const response = await fetch(url, { headers: { Accept: "text/html" }, signal: pending.signal });
      if (!response.ok) throw new Error(`Status ${response.status}`);
      const page = new DOMParser().parseFromString(await response.text(), "text/html");
      const next = page.getElementById("main");
      if (!main || !next || !next.querySelector("[data-swap]")) throw new Error("Not a page with a Window");
      if (mine !== turn) return;
      const open = [...main.querySelectorAll("details[id][open]")].map((details) => details.id);
      const { scrollX, scrollY } = window;
      main.replaceChildren(...document.adoptNode(next).childNodes);
      for (const id of open) document.getElementById(id)?.setAttribute("open", "");
      document.title = page.title;
      if (push) history.pushState(null, "", url);
      shown = address();
      window.scrollTo(scrollX, scrollY);
      if (push) {
        const target = new URL(url, location.href);
        const href = target.pathname + target.search + target.hash;
        document.querySelector(`a[data-swap][href="${CSS.escape(href)}"]`)?.focus({ preventScroll: true });
      }
    } catch {
      if (mine === turn) location.assign(url);
    } finally {
      if (mine === turn) {
        pending = undefined;
        main?.removeAttribute("aria-busy");
      }
    }
  }

  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest("a[data-swap]");
    if (!link || link.origin !== location.origin) return;
    event.preventDefault();
    show(link.href, true);
  });

  addEventListener("popstate", () => {
    if (!pending && address() === shown) return;
    show(location.href);
  });
})();
