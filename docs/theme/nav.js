/* Collapsible TOC sidebar. Vanilla ES6, no dependencies.
 *
 * Progressive enhancement over Asciidoctor's static TOC:
 * without JS the full TOC renders and every anchor link works.
 * With JS, nodes with children get a disclosure button:
 * clicking the button (not the title link) expands/collapses.
 *
 * Behaviour (matches our other manuals):
 * - The tree loads fully expanded on every visit: no minimized
 *   default, no saved collapsed state.
 * - "Collapse all" / "Expand all" controls sit above the list for
 *   readers who want a shorter sidebar; toggling is session-local.
 * - The hierarchy containing location.hash is auto-expanded on load
 *   and on hash navigation (a no-op unless the reader collapsed it).
 * - The scroll-spy highlights the current section only -- it never
 *   expands or collapses entries.
 */
(function () {
  'use strict';

  var TOC_ID = 'toc';
  var COLLAPSED_CLASS = 'toc-collapsed';
  var ACTIVE_CLASS = 'toc-active';

  function setCollapsed(li, toggle, collapsed) {
    li.classList.toggle(COLLAPSED_CLASS, collapsed);
    toggle.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
  }

  // Auto-expand every ancestor of the current section so deep links
  // always reveal where the reader is.
  function expandAncestorsOf(link) {
    var el = link;
    while (el) {
      if (el.tagName === 'LI' && el.classList.contains(COLLAPSED_CLASS)) {
        el.classList.remove(COLLAPSED_CLASS);
        var toggle = el.querySelector(':scope > button.toc-toggle');
        if (toggle) toggle.setAttribute('aria-expanded', 'true');
      }
      el = el.parentElement;
    }
  }

  function linkFor(hash) {
    if (!hash) return null;
    // Guard the selector against quotes in section ids.
    return document.querySelector(
      '#' + TOC_ID + ' a[href="' + hash.replace(/"/g, '\\"') + '"]'
    );
  }

  function setActive(link) {
    document.querySelectorAll('#' + TOC_ID + ' a.' + ACTIVE_CLASS).forEach(function (a) {
      a.classList.remove(ACTIVE_CLASS);
    });
    if (link) link.classList.add(ACTIVE_CLASS);
  }

  function revealHash(hash) {
    var link = linkFor(hash);
    if (link) {
      expandAncestorsOf(link);
      setActive(link);
    }
  }

  function addToolbar(toc) {
    var bar = document.createElement('div');
    bar.className = 'toc-toolbar';

    var collapseAll = document.createElement('button');
    collapseAll.type = 'button';
    collapseAll.textContent = 'Collapse all';
    collapseAll.addEventListener('click', function () {
      toc.querySelectorAll('li > button.toc-toggle').forEach(function (toggle) {
        setCollapsed(toggle.parentElement, toggle, true);
      });
    });

    var expandAll = document.createElement('button');
    expandAll.type = 'button';
    expandAll.textContent = 'Expand all';
    expandAll.addEventListener('click', function () {
      toc.querySelectorAll('li > button.toc-toggle').forEach(function (toggle) {
        setCollapsed(toggle.parentElement, toggle, false);
      });
    });

    bar.appendChild(collapseAll);
    bar.appendChild(expandAll);
    toc.insertBefore(bar, toc.firstChild);
  }

  function init() {
    var toc = document.getElementById(TOC_ID);
    if (!toc) return;

    // Attach a disclosure button to every TOC node that has children.
    // The title link itself is untouched, so navigation still works
    // normally (click title = navigate; click arrow = expand/collapse).
    // Initial state: always fully expanded.
    toc.querySelectorAll('li').forEach(function (li) {
      var childList = li.querySelector(':scope > ul, :scope > ol');
      if (!childList) return;
      var link = li.querySelector(':scope > a');
      if (!link) return;

      var toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'toc-toggle';
      toggle.setAttribute('aria-label', 'Expand or collapse: ' + link.textContent.trim());
      setCollapsed(li, toggle, false);

      toggle.addEventListener('click', function () {
        setCollapsed(li, toggle, !li.classList.contains(COLLAPSED_CLASS));
      });

      li.insertBefore(toggle, link);
    });

    addToolbar(toc);

    // Reveal and highlight the navigated-to section on load and on navigation.
    revealHash(window.location.hash);
    window.addEventListener('hashchange', function () {
      revealHash(window.location.hash);
    });

    // Scroll-spy: highlight the section in view while scrolling.
    // It never expands or collapses entries.
    if ('IntersectionObserver' in window) {
      var headings = document.querySelectorAll('#content [id]');
      var observer = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (entry) {
            if (entry.isIntersecting) setActive(linkFor('#' + entry.target.id));
          });
        },
        { rootMargin: '0px 0px -70% 0px' }
      );
      headings.forEach(function (h) {
        observer.observe(h);
      });
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
