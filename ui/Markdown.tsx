import React,{useEffect,useId,useMemo,useState} from 'react';
import ReactMarkdown,{type Components} from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import hljs from 'highlight.js';
import DOMPurify from 'dompurify';
import {invoke} from '@tauri-apps/api/core';
import 'highlight.js/styles/github-dark.css';

function Diagram({source}:{source:string}){
 const id=useId().replace(/[^a-zA-Z0-9]/g,''),[svg,setSvg]=useState(''),[error,setError]=useState(''),[theme,setTheme]=useState(document.documentElement.dataset.theme||'dark');
 useEffect(()=>{const update=()=>setTheme(document.documentElement.dataset.theme||'dark');window.addEventListener('struve-theme',update);return()=>window.removeEventListener('struve-theme',update)},[]);
 useEffect(()=>{let live=true;setSvg('');setError('');void (async()=>{try{
   if(source.length>20000)throw Error('Diagram exceeds 20,000 characters');
   const {default:mermaid}=await import('mermaid');
   mermaid.initialize({startOnLoad:false,securityLevel:'strict',theme:theme==='light'?'default':'dark',suppressErrorRendering:true,htmlLabels:false,
     flowchart:{htmlLabels:false},maxTextSize:20000,secure:['securityLevel','startOnLoad','maxTextSize','maxEdges','htmlLabels']});
   const rendered=await mermaid.render('diagram'+id,source);
   if(live)setSvg(DOMPurify.sanitize(rendered.svg,{USE_PROFILES:{svg:true,svgFilters:true}}));
 }catch(e){if(live)setError(String(e))}})();return()=>{live=false}},[source,id,theme]);
 return error?<div className="diagram-error"><small>{error}</small><pre><code>{source}</code></pre></div>:svg?<div className="mermaid-diagram" dangerouslySetInnerHTML={{__html:svg}}/>:<div className="diagram-loading">Mermaid…</div>;
}
function decodedPath(value:string){try{return decodeURIComponent(value)}catch{return value}}
function DocumentImage({src,alt,basePath}:{src?:string,alt?:string,basePath?:string}){
 const [image,setImage]=useState(''),[error,setError]=useState('');
 useEffect(()=>{let live=true;setImage('');setError('');if(basePath&&src&&!/^[a-z]+:/i.test(src)&&!src.startsWith('/'))void invoke<string>('preview_image',{basePath,relativePath:decodedPath(src)}).then(data=>{if(live)setImage(data)}).catch(e=>{if(live)setError(String(e))});return()=>{live=false}},[src,basePath]);
 if(image)return <img className="markdown-image" src={image} alt={alt||''} loading="lazy"/>;
 return <span className="markdown-image-note" title={error||src}>🖼 {alt||'Image'}{src&&/^https?:\/\//i.test(src)&&<a href={src} onClick={e=>{e.preventDefault();window.dispatchEvent(new CustomEvent('struve-open-url',{detail:src}))}}> ↗</a>}{error&&<small> · {error}</small>}</span>;
}
export default React.memo(function Markdown({content,basePath,onSource}:{content:string,basePath?:string,onSource?:(href:string)=>void}){
 const components=useMemo<Components>(()=>({
   code({className,children,...props}){if(className?.includes('language-mermaid'))return <Diagram source={String(children).trim()}/>;return <code className={className} {...props}>{children}</code>},
   a({children,href}){return <a href={href} onClick={e=>{e.preventDefault();if(href?.startsWith('#/source/')||href?.startsWith('#/library/'))onSource?.(href);else if(href&&/^https?:\/\//i.test(href))window.dispatchEvent(new CustomEvent('struve-open-url',{detail:href}))}}>{children}</a>},
   img({alt,src}){return <DocumentImage src={src} alt={alt} basePath={basePath}/>},
 }),[basePath,onSource]);
 return <div className="markdown-body"><ReactMarkdown remarkPlugins={[remarkGfm]} rehypePlugins={[[rehypeHighlight,{detect:false}]]} skipHtml components={components}>{content}</ReactMarkdown></div>;
});
const aliases:Record<string,string>={rs:'rust',py:'python',js:'javascript',mjs:'javascript',cjs:'javascript',ts:'typescript',tsx:'typescript',jsx:'javascript',h:'cpp',hpp:'cpp',cc:'cpp',cxx:'cpp',md:'markdown',ps1:'powershell',yml:'yaml',sh:'bash',bat:'dos',cmd:'dos',html:'xml',svg:'xml',cs:'csharp'};
export function SourceCode({content,format,wrap}:{content:string,format:string,wrap:boolean}){
 const preview=content.slice(0,250000),truncated=preview.length<content.length;
 const highlighted=useMemo(()=>{const language=aliases[format]||format;try{return preview.length<=200000&&hljs.getLanguage(language)?hljs.highlight(preview,{language,ignoreIllegals:true}).value:null}catch{return null}},[preview,format]);
 return <>{truncated&&<div className="preview-note">{document.documentElement.lang==='en'?'Preview limited to 250,000 characters; original file is unchanged.':'Показаны первые 250 000 символов; исходный файл сохранён целиком.'}</div>}<div className={'source-code '+(wrap?'wrap':'')}><div className="source-numbers" aria-hidden="true">{preview.split('\n').map((_,i)=><div key={i}>{i+1}</div>)}</div><pre>{highlighted?<code className="hljs" dangerouslySetInnerHTML={{__html:highlighted}}/>:<code>{preview}</code>}</pre></div></>;
}
