/* Collapsible TOC sidebar. Vanilla ES6, no dependencies.
 *
 * Progressive enhancement over Asciidoctor's static TOC:
 * without JS the full TOC renders and every anchor link works.
 * With JS, nodes with children get a disclosure button:
 * clicking the button (not the title link) expands/collapses.
 */
(function () {
  'use strict';

  var TOC_ID = 'toc';
  var STORAGE_KEY = 'project-docs.nav.v1';
  var COLLAPSED_CLASS = 'toc-collapsed';
  var ACTIVE_CLASS = 'toc-active';

  // localStorage may throw (private mode, disabled cookies): fail open
  // with an in-memory stub so navigation never breaks.
  function loadState() {
    try {
      var raw = window.localStorage.getItem(STORAGE_KEY);
      return raw ? JSON.parse(raw) : {};
    } catch (e) {
      return {};
    }
  }

  function saveState(state) {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    } catch (e) {
      /* non-fatal: collapse state simply won't persist */
    }
  }

  // Stable key per collapsible node: the section anchor it links to.
  function nodeKey(li) {
    var link = li.querySelector(':scope > a[href^="#"]');
    return link ? link.getAttribute('href') : null;
  }

  function setCollapsed(li, toggle, collapsed) {
    li.classList.toggle(COLLAPSED_CLASS, collapsed);
    toggle.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
  }

  function collectState(toc) {
    var state = {};
    toc.querySelectorAll('li.' + COLLAPSED_CLASS).forEach(function (li) {
      var key = nodeKey(li);
      if (key) state[key] = true;
    });
    return state;
  }

  // Auto-expand every ancestor of the current section so deep links and
  // refreshes always reveal where the reader is.
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

  function markActive(toc, hash) {
    toc.querySelectorAll('a.' + ACTIVE_CLASS).forEach(function (a) {
      a.classList.remove(ACTIVE_CLASS);
    });
    if (!hash) return;
    // CSS.escape handles section ids with special characters.
    var link = toc.querySelector('a[href="' + hash.replace(/"/g, '\\"') + '"]');
    if (link) {
      link.classList.add(ACTIVE_CLASS);
      expandAncestorsOf(link);
    }
  }

  function addToolbar(toc, state) {
    var bar = document.createElement('div');
    bar.className = 'toc-toolbar';

    var collapseAll = document.createElement('button');
    collapseAll.type = 'button';
    collapseAll.textContent = 'Collapse all';
    collapseAll.addEventListener('click', function () {
      toc.querySelectorAll('li > button.toc-toggle').forEach(function (toggle) {
        var li = toggle.parentElement;
        setCollapsed(li, toggle, true);
      });
      saveState(collectState(toc));
    });

    var expandAll = document.createElement('button');
    expandAll.type = 'button';
    expandAll.textContent = 'Expand all';
    expandAll.addEventListener('click', function () {
      toc.querySelectorAll('li > button.toc-toggle').forEach(function (toggle) {
        var li = toggle.parentElement;
        setCollapsed(li, toggle, false);
      });
      saveState(collectState(toc));
    });

    bar.appendChild(collapseAll);
    bar.appendChild(expandAll);
    toc.insertBefore(bar, toc.firstChild);
    void state; // toolbar is stateless; persisted map stays authoritative
  }

  function init() {
    var toc = document.getElementById(TOC_ID);
    if (!toc) return;

    var state = loadState();

    // Attach a disclosure button to every TOC node that has children.
    // The title link itself is untouched, so navigation still works
    // normally (click title = navigate; click arrow = expand/collapse).
    toc.querySelectorAll('li').forEach(function (li) {
      var childList = li.querySelector(':scope > ul, :scope > ol');
      if (!childList) return;
      var link = li.querySelector(':scope > a');
      if (!link) return;

      var toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'toc-toggle';
      toggle.setAttribute('aria-label', 'Expand or collapse: ' + link.textContent.trim());

      var key = nodeKey(li);
      setCollapsed(li, toggle, !!(key && state[key]));

      toggle.addEventListener('click', function () {
        var collapsed = li.classList.contains(COLLAPSED_CLASS);
        setCollapsed(li, toggle, !collapsed);
        saveState(collectState(toc));
      });

      li.insertBefore(toggle, link);
    });

    addToolbar(toc, state);

    // Reveal and highlight the current section on load and on navigation.
    markActive(toc, window.location.hash);
    if (window.location.hash) saveState(collectState(toc));
    window.addEventListener('hashchange', function () {
      markActive(toc, window.location.hash);
      saveState(collectState(toc));
    });

    // Subtly track the section in view while scrolling.
    if ('IntersectionObserver' in window) {
      var headings = document.querySelectorAll('#content [id]');
      var observer = new IntersectionObserver(
        function (entries) {
          entries.forEach(function (entry) {
            if (entry.isIntersecting) markActive(toc, '#' + entry.target.id);
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
