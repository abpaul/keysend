(() => {
  const open = () => document.querySelectorAll("details.explain[open], details.lang-menu[open]");

  document.addEventListener("click", (event) => {
    for (const details of open()) if (!details.contains(event.target)) details.removeAttribute("open");
  });

  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    for (const details of open()) {
      const focused = details.contains(document.activeElement);
      details.removeAttribute("open");
      if (focused) details.querySelector("summary").focus();
    }
  });
})();
