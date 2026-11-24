import React,{useMemo} from 'react';
import DOMPurify from 'dompurify';
export default function Document({html,warnings,lang}:{html:string,warnings?:string[],lang:string}){
 const content=useMemo(()=>{
  const fragment=DOMPurify.sanitize(html,{USE_PROFILES:{html:true},FORBID_TAGS:['style','form','input','button','video','audio','iframe'],FORBID_ATTR:['style','srcset'],RETURN_DOM_FRAGMENT:true});
  for(const image of fragment.querySelectorAll('img'))if(!/^data:image\/(png|jpeg|gif|webp|bmp);base64,/i.test(image.getAttribute('src')||''))image.removeAttribute('src');
  for(const link of fragment.querySelectorAll('a'))if(!/^(https?:\/\/|#)/i.test(link.getAttribute('href')||''))link.removeAttribute('href');
  const container=document.createElement('div');container.appendChild(fragment);return container.innerHTML;
 },[html]);
 function openLink(e:React.MouseEvent){
  const a=(e.target as HTMLElement).closest('a');if(!a)return;
  const href=a.getAttribute('href');if(href?.startsWith('#'))return;
  e.preventDefault();if(href&&/^https?:\/\//i.test(href))window.dispatchEvent(new CustomEvent('struve-open-url',{detail:href}));
 }
 return <div className="rich-document"><div className="preview-note">{lang==='en'?'DOCX · content preview · page layout may differ':'DOCX · просмотр содержимого · разбивка на страницы может отличаться'}</div>{!!warnings?.length&&<details className="preview-note"><summary>{lang==='en'?'Conversion notes':'Примечания преобразования'} · {warnings.length}</summary>{warnings.map((w,i)=><p key={i}>{w}</p>)}</details>}<div className="markdown-body" onClick={openLink} dangerouslySetInnerHTML={{__html:content}}/></div>;
}
