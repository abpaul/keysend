(() => {
  const address = () => location.pathname + location.search;
  let shown = address();
  let turn = 0;

  async function show(url) {
    const mine = ++turn;
    const response = await fetch(url, { headers: { Accept: "text/html" } });
    if (!response.ok) throw new Error(`Status ${response.status}`);
    const page = new DOMParser().parseFromString(await response.text(), "text/html");
    const next = page.getElementById("main");
    const main = document.getElementById("main");
    if (!main || !next || !next.querySelector("[data-swap]")) throw new Error("Not a page with a Window");
    if (mine !== turn) return false;
    const open = [...main.querySelectorAll("details[id][open]")].map((details) => details.id);
    const { scrollX, scrollY } = window;
    main.replaceChildren(...document.adoptNode(next).childNodes);
    for (const id of open) document.getElementById(id)?.setAttribute("open", "");
    document.title = page.title;
    window.scrollTo(scrollX, scrollY);
    return true;
  }

  document.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest("a[data-swap]");
    if (!link || link.origin !== location.origin) return;
    event.preventDefault();
    const href = link.getAttribute("href");
    show(link.href).then((swapped) => {
      if (!swapped) return;
      history.pushState(null, "", link.href);
      shown = address();
      document.querySelector(`a[data-swap][href="${CSS.escape(href)}"]`)?.focus({ preventScroll: true });
    }).catch(() => location.assign(link.href));
  });

  addEventListener("popstate", () => {
    if (address() === shown) return;
    shown = address();
    show(location.href).catch(() => location.reload());
  });
})();
