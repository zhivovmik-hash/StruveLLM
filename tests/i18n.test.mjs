import test from 'node:test';import assert from 'node:assert/strict';import ts from 'typescript';import fs from 'node:fs';
const source=fs.readFileSync(new URL('../ui/i18n.ts',import.meta.url),'utf8');const code=ts.transpileModule(source,{compilerOptions:{module:ts.ModuleKind.ES2022}}).outputText;const mod=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
test('all static UI phrases have English translations',()=>{const ui=fs.readFileSync(new URL('../ui/main.tsx',import.meta.url),'utf8');const ast=ts.createSourceFile('main.tsx',ui,ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);const missing=[];mod.setLanguage('en');function visit(n){if(ts.isCallExpression(n)&&n.expression.getText(ast)==='tr'&&ts.isStringLiteral(n.arguments[0])){const s=n.arguments[0].text;if(/[А-Яа-яЁё]/.test(mod.tr(s))&&!s.includes('Interface language'))missing.push(s)}ts.forEachChild(n,visit)}visit(ast);assert.deepEqual(missing,[]);mod.setLanguage('ru');assert.equal(mod.tr('Ресурсы'),'Ресурсы');mod.setLanguage('en');assert.equal(mod.tr('Ресурсы'),'Resources')});
test('resource validation errors follow the selected language',()=>{mod.setLanguage('en');assert.equal(mod.translateError('GPU-test: устройство отсутствует. Выберите GPU из текущей конфигурации'),'GPU-test: device is missing. Select a GPU from the current configuration');mod.setLanguage('ru');assert.equal(mod.translateError('Models directory must exist'),'Каталог моделей должен существовать')});
test('first render uses the locale supplied by the desktop before the controller replies',async()=>{
 for(const [locale,label] of [['en','Resources'],['ru','Ресурсы']]){
  globalThis.__STRUVE_INITIAL_LANGUAGE__=locale;
  try{
   const fresh=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64')+'#first-'+locale);
   assert.equal(fresh.language,locale);assert.equal(fresh.tr('Ресурсы'),label);
  }finally{delete globalThis.__STRUVE_INITIAL_LANGUAGE__}
 }
});
