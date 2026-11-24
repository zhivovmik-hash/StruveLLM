(() => {
  if (document.readyState === 'loading') return {state:'loading'};
  const isGoogle = location.hostname === 'www.google.com';
  const list = [];
  const seen = new Set();
  for (const heading of document.querySelectorAll(isGoogle ? 'h3' : 'h2,h3')) {
    if (!heading.getClientRects().length || heading.closest('[data-text-ad], [data-ad-client], [aria-label="Ads"]')) continue;
    const anchor = heading.closest('a[href]') || heading.querySelector('a[href]');
    if (!anchor) continue;
    try {
      let url = new URL(anchor.href);
      let redirect = false;
      if (isGoogle && url.origin === location.origin && ['/url','/goto'].includes(url.pathname)) {
        const target=url.searchParams.get('q') || url.searchParams.get('url') || '';
        if (/^https?:\/\//i.test(target)) url=new URL(target);
        else redirect=Boolean(target);
      }
      if (!['https:','http:'].includes(url.protocol) || url.username || url.password ||
          (url.hostname === location.hostname && !redirect) || seen.has(url.href) || url.href.length > 3000) continue;
      const title = (heading.innerText || '').replace(/\s+/g,' ').trim().slice(0,240);
      if (title) {seen.add(url.href); list.push(redirect ? {title,redirect:url.href} : {title,url:url.href});}
      if (list.length === 6) break;
    } catch {}
  }
  if(list.length) return {state:'results',results:list};
  const text = (document.body?.innerText || '').slice(0,3000);
  if (/unusual traffic|подозрительный трафик|подтвердите, что|вы не робот|i.m not a robot|are you a robot/i.test(text)) return {state:'verification'};
  if (/did not match any documents|по вашему запросу ничего не (?:нашлось|найдено)|ничего не нашлось/i.test(text)) return {state:'empty'};
  return {state:'waiting'};
})()
