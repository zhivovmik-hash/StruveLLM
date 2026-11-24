import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {McpServer} from '@modelcontextprotocol/sdk/server/mcp.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {z} from 'zod';

export function projectPath(relative, {write=false, root=process.env.STRUVE_PROJECT_ROOT}={}) {
  if(!root) throw Error('Select a project before using document tools');
  const base=fs.realpathSync(root), candidate=path.resolve(base,relative);
  const inside=p=>{const rel=path.relative(base,p);return rel!==''&&!rel.startsWith('..'+path.sep)&&rel!=='..'&&!path.isAbsolute(rel)};
  const parts=path.relative(base,candidate).toLowerCase().split(path.sep);
  if(parts.some(p=>['.git','.codex','.aws','.ssh','node_modules'].includes(p))||parts.at(-1)?.startsWith('.env'))throw Error('Application credentials and internal configuration are not document inputs');
  if(process.env.STRUVE_APP_ROOT){const internal=path.relative(path.resolve(process.env.STRUVE_APP_ROOT,'data'),candidate);if(internal===''||(!internal.startsWith('..')&&!path.isAbsolute(internal)))throw Error('Application data is outside the document workspace');}
  if(!inside(candidate))throw Error('Path must point to a file inside the selected project');
  if(write){const parent=fs.realpathSync(path.dirname(candidate));if(parent!==base&&!inside(parent))throw Error('Output parent leaves the project');if(fs.existsSync(candidate))throw Error('Output already exists; choose a new file name');}
  else {if(!inside(fs.realpathSync(candidate)))throw Error('File link leaves the project');if(!fs.statSync(candidate).isFile())throw Error('Expected a file');}
  return candidate;
}
export async function sizeCheck(file){
 if(fs.statSync(file).size>30*1024*1024)throw Error('Document limit: 30 MiB');
 if(['.docx','.xlsx'].includes(path.extname(file).toLowerCase())){
  const {default:yauzl}=await import('yauzl');
  await new Promise((resolve,reject)=>yauzl.open(file,{lazyEntries:true},(error,zip)=>{
   if(error)return reject(error);let bytes=0,entries=0;zip.on('error',reject);zip.on('end',resolve);zip.on('entry',entry=>{bytes+=entry.uncompressedSize;entries++;if(bytes>128*1024*1024||entries>20000){zip.close();reject(Error('Document archive exceeds 128 MiB expanded or 20,000 entries'));}else zip.readEntry();});zip.readEntry();
  }));
 }
}
export async function readDocument(file,{maxCharacters=60000,maxRows=100,maxPages=30,startCharacter=0,startRow=1,startPage=1,sheetName}={}) {
  await sizeCheck(file);const ext=path.extname(file).toLowerCase();let result;
  if(ext==='.docx'){
    const {default:mammoth}=await import('mammoth');const out=await mammoth.extractRawText({path:file});
    result={format:'docx',text:out.value,warnings:out.messages.map(m=>m.message)};
  } else if(ext==='.xlsx'){
    const {default:ExcelJS}=await import('exceljs');const book=new ExcelJS.Workbook();await book.xlsx.readFile(file);
    const selected=sheetName?book.worksheets.filter(s=>s.name===sheetName):book.worksheets;
    if(sheetName&&!selected.length)throw Error('Unknown sheet: '+sheetName);
    const sheets=selected.map(sheet=>({name:sheet.name,state:sheet.state,rows:sheet.rowCount,columns:sheet.columnCount,start_row:startRow,
      data:Array.from({length:Math.max(0,Math.min(sheet.rowCount-startRow+1,maxRows))},(_,i)=>({row:startRow+i,cells:sheet.getRow(startRow+i).values.slice(1,65).map(value=>value??null)})),
      next_row:sheet.rowCount>=startRow+maxRows?startRow+maxRows:null,
      truncated:sheet.rowCount>=startRow+maxRows||sheet.columnCount>64}));
    result={format:'xlsx',sheets,text:sheets.map(s=>`Sheet: ${s.name} (${s.state})\n${s.data.map(r=>JSON.stringify(r)).join('\n')}`).join('\n\n'),warning:'Formula results are cached values; this tool does not recalculate formulas.'};
  } else if(ext==='.pdf'){
    const pdfjs=await import('pdfjs-dist/legacy/build/pdf.mjs');
    const document=await pdfjs.getDocument({data:new Uint8Array(fs.readFileSync(file)),useSystemFonts:true,isEvalSupported:false,verbosity:0}).promise;
    const pages=[];try{if(startPage>document.numPages)throw Error('Start page exceeds page count');for(let i=startPage;i<=Math.min(document.numPages,startPage+maxPages-1);i++){const page=await document.getPage(i);const content=await page.getTextContent();pages.push({page:i,text:content.items.map(t=>t.str+(t.hasEOL?'\n':' ')).join('')});page.cleanup();}
      result={format:'pdf',page_count:document.numPages,start_page:startPage,pages,text:pages.map(p=>`[Page ${p.page}]\n${p.text}`).join('\n'),pages_truncated:document.numPages>=startPage+maxPages,next_page:document.numPages>=startPage+maxPages?startPage+maxPages:null};
      if(!result.text.replace(/\[Page \d+\]/g,'').trim())result.warning='No text layer found. OCR is not included; use a vision model for page images.';
    }finally{await document.destroy();}
  } else {
    const bytes=fs.readFileSync(file);if(bytes.includes(0))throw Error('Unsupported binary document');
    result={format:ext.slice(1)||'text',text:new TextDecoder('utf-8',{fatal:true}).decode(bytes)};
  }
  const textLength=result.text.length;
  if(startCharacter>textLength)throw Error('Start character exceeds extracted text length');
  if(startCharacter>0&&/[\uDC00-\uDFFF]/.test(result.text[startCharacter]||''))throw Error('Start character splits a Unicode character; use the returned next_character');
  let end=Math.min(textLength,startCharacter+maxCharacters);
  if(end<textLength&&/[\uD800-\uDBFF]/.test(result.text[end-1]||''))end--;
  result.text=result.text.slice(startCharacter,end);
  result.text_range={start:startCharacter,end,total_characters:textLength,next_character:end<textLength?end:null};
  if(end<textLength)result.truncated=true;
  return result;
}
async function createDocx(file,args){
  const {Document,Packer,Paragraph,TextRun,HeadingLevel,Table,TableRow,TableCell}=await import('docx');
  const children=[];
  if(args.title)children.push(new Paragraph({text:args.title,heading:HeadingLevel.TITLE}));
  for(const block of args.paragraphs){children.push(new Paragraph({children:[new TextRun({text:block.text,bold:!!block.bold})],heading:block.heading?HeadingLevel['HEADING_'+block.heading]:undefined}));}
  if(args.table?.length)children.push(new Table({rows:args.table.map(row=>new TableRow({children:row.map(text=>new TableCell({children:[new Paragraph(String(text))]}))}))}));
  const doc=new Document({sections:[{children}]});fs.writeFileSync(file,await Packer.toBuffer(doc),{flag:'wx'});
}
async function createXlsx(file,args){
  const {default:ExcelJS}=await import('exceljs');const book=new ExcelJS.Workbook();book.creator='Struve LLM';
  for(const table of args.sheets){const sheet=book.addWorksheet(table.name);sheet.addRows(table.rows);if(table.header){const row=sheet.getRow(1);row.font={bold:true};row.fill={type:'pattern',pattern:'solid',fgColor:{argb:'FFE5EBDD'}};sheet.views=[{state:'frozen',ySplit:1}];}sheet.columns.forEach(column=>{column.width=24});}
  const bytes=await book.xlsx.writeBuffer();fs.writeFileSync(file,bytes,{flag:'wx'});
}
async function createPdf(file,args){
  const {PDFDocument,rgb}=await import('pdf-lib');const {default:fontkit}=await import('@pdf-lib/fontkit');
  const doc=await PDFDocument.create();doc.registerFontkit(fontkit);
  const fontFile=path.join(process.env.SystemRoot||'C:\\Windows','Fonts','segoeui.ttf');
  const font=await doc.embedFont(fs.readFileSync(fontFile),{subset:true});let page=doc.addPage([595.28,841.89]),y=790;
  function draw(text,size){const words=text.split(/\s+/);let line='';for(const word of words){if(font.widthOfTextAtSize((line+' '+word).trim(),size)>490&&line){nextLine(line,size);line=word}else line=(line+' '+word).trim();}if(line)nextLine(line,size);y-=size*.5;}
  function nextLine(text,size){if(y<55){page=doc.addPage([595.28,841.89]);y=790;}page.drawText(text,{x:52,y,size,font,color:rgb(.12,.14,.16)});y-=size*1.5;}
  if(args.title)draw(args.title,20);for(const paragraph of args.paragraphs)draw(paragraph,11);
  fs.writeFileSync(file,await doc.save(),{flag:'wx'});
}
export async function startDocumentServer(){
 const server=new McpServer({name:'struve-documents',version:'0.3.5'});
 const text=z.string().max(100000),rel=z.string().min(1).max(1000);
 const answer=value=>({content:[{type:'text',text:JSON.stringify(value)}]});
 const wrap=fn=>async args=>{try{return answer(await fn(args))}catch(e){return {...answer({error:e.message}),isError:true}}};
 server.registerTool('document_read',{description:'Read project DOCX/PDF/XLSX or UTF-8 code/Markdown. Continue using next_character (zero-based), next_page or next_row (one-based); XLSX supports sheet selection. Formula values are cached.',inputSchema:{path:rel,max_characters:z.number().int().min(1000).max(120000).default(60000),start_character:z.number().int().min(0).default(0),max_rows:z.number().int().min(1).max(500).default(100),start_row:z.number().int().min(1).default(1),sheet:z.string().max(31).optional(),max_pages:z.number().int().min(1).max(100).default(30),start_page:z.number().int().min(1).default(1)},annotations:{readOnlyHint:true,destructiveHint:false}},wrap(async a=>readDocument(projectPath(a.path),{maxCharacters:a.max_characters,maxRows:a.max_rows,maxPages:a.max_pages,startCharacter:a.start_character,startRow:a.start_row,startPage:a.start_page,sheetName:a.sheet})));
 server.registerTool('docx_create',{description:'Create a new DOCX with paragraphs, headings and optional table. Existing files are never overwritten.',inputSchema:{path:rel,title:text.optional(),paragraphs:z.array(z.object({text,heading:z.number().int().min(1).max(6).optional(),bold:z.boolean().optional()})).max(300),table:z.array(z.array(text).max(30)).max(300).optional()},annotations:{readOnlyHint:false,destructiveHint:false}},wrap(async a=>{const file=projectPath(a.path,{write:true});if(!file.toLowerCase().endsWith('.docx'))throw Error('Use .docx extension');await createDocx(file,a);return {created:file,bytes:fs.statSync(file).size}}));
 server.registerTool('xlsx_create',{description:'Create a new XLSX workbook. Rows contain literal values; formula strings are not executed.',inputSchema:{path:rel,sheets:z.array(z.object({name:z.string().min(1).max(31),header:z.boolean().default(true).describe('If true, the FIRST row in rows contains your column headings. Include that heading row explicitly; labels are not generated automatically.'),rows:z.array(z.array(z.union([z.string().max(32000),z.number(),z.boolean(),z.null()])).max(100)).max(2000)})).min(1).max(10)},annotations:{readOnlyHint:false,destructiveHint:false}},wrap(async a=>{const file=projectPath(a.path,{write:true});if(!file.toLowerCase().endsWith('.xlsx'))throw Error('Use .xlsx extension');await createXlsx(file,a);return {created:file,bytes:fs.statSync(file).size}}));
 server.registerTool('xlsx_edit_copy',{description:'Edit specified cells in a COPY of an XLSX workbook, preserving the original. Formula calculation is not included.',inputSchema:{source:rel,output:rel,changes:z.array(z.object({sheet:z.string(),cell:z.string().regex(/^[A-Z]{1,3}[1-9][0-9]{0,5}$/),value:z.union([z.string().max(32000),z.number(),z.boolean(),z.null()])})).min(1).max(1000)},annotations:{readOnlyHint:false,destructiveHint:false}},wrap(async a=>{const source=projectPath(a.source),output=projectPath(a.output,{write:true});if(!source.toLowerCase().endsWith('.xlsx')||!output.toLowerCase().endsWith('.xlsx'))throw Error('Use .xlsx files');await sizeCheck(source);const {default:ExcelJS}=await import('exceljs');const book=new ExcelJS.Workbook();await book.xlsx.readFile(source);for(const change of a.changes){const sheet=book.getWorksheet(change.sheet);if(!sheet)throw Error('Unknown sheet: '+change.sheet);sheet.getCell(change.cell).value=change.value;}fs.writeFileSync(output,await book.xlsx.writeBuffer(),{flag:'wx'});return {created:output,changed_cells:a.changes.length}}));
 server.registerTool('pdf_create',{description:'Create a new paginated PDF from plain paragraphs, with Cyrillic text supported.',inputSchema:{path:rel,title:text.optional(),paragraphs:z.array(text).max(500)},annotations:{readOnlyHint:false,destructiveHint:false}},wrap(async a=>{const file=projectPath(a.path,{write:true});if(!file.toLowerCase().endsWith('.pdf'))throw Error('Use .pdf extension');await createPdf(file,a);return {created:file,bytes:fs.statSync(file).size}}));
 await server.connect(new StdioServerTransport());
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href)await startDocumentServer();
