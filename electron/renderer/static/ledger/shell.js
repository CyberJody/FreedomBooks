/* Hybrid UI v1 shell: on narrow screens the navigation is a horizontally scrolling strip; bring the
   current entry into view. Progressive enhancement only — navigation works without it. */
document.addEventListener('DOMContentLoaded', function () {
  var nav = document.querySelector('nav.appnav');
  var current = nav && nav.querySelector('a[aria-current="page"]');
  if (!nav || !current || nav.scrollWidth <= nav.clientWidth) return;
  nav.scrollLeft = Math.max(0, current.offsetLeft - nav.offsetLeft - 8);
});
