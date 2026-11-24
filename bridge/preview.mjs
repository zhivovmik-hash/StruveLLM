import {readDocument} from './documents.mjs';
import {previewDocx} from './document-preview.mjs';
try{let result;if(process.argv[2].toLowerCase().endsWith('.docx'))result=await previewDocx(process.argv[2]);else{const doc=await readDocument(process.argv[2],{maxRows:200,maxCharacters:120000});result={sheets:doc.sheets?.slice(0,50),sheets_truncated:doc.sheets?.length>50};}process.stdout.write(JSON.stringify(result));}catch(error){console.error(error.message);process.exitCode=1;}
