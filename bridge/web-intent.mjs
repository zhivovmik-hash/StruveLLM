const action=/(?:найд(?:и|ите)|поищ(?:и|ите)|ищи|ищите|выполни(?:те)? поиск|проверь(?:те)?|посмотри(?:те)?|search|look\s*up|find|check|погугли|загугли)/iu;
const online=/(?:интернет|в\s+сети|гугл|яндекс|google|yandex|сайт|онлайн|web\b|online\b|website)/iu;
const refuse=/(?:без (?:поиска|интернета)|не (?:ищи|ищите|гугли|пользуйся интернетом)|don['’]t (?:search|browse)|do not (?:search|browse)|without (?:web|internet))/iu;
export function readIntentURL(text){
  if(refuse.test(text)||/(?:не (?:читай|открывай)|(?:don't|do not) (?:read|open))/iu.test(text))return '';
  if(!/(?:прочитай|прочтите|открой|изучи|посети|перескажи|\bread\b|\bvisit\b|summari[sz]e)/iu.test(text))return '';
  const match=text.match(/https?:\/\/[^\s<>"«»]+/iu);
  if(!match)return '';
  try{const url=new URL(match[0].replace(/[.,;:!?\])]+$/u,''));return url.username||url.password?'':url.href}catch{return ''}
}
const retry=/(?:ищи лучше|поищи (?:ещ[её]|снова)|search again|try searching again|не тот сайт|другой сайт|wrong (?:site|website))/iu;
export function previousWebRequest(userTexts){
  for(const text of userTexts.slice(-4).reverse()){
    if(refuse.test(text))break;
    if(retry.test(text))continue;
    return action.test(text)&&online.test(text)?text:'';
  }
  return '';
}
export function webIntent(text,previous=''){
  if(refuse.test(text))return false;
  return retry.test(text)?action.test(previous)&&online.test(previous):action.test(text)&&online.test(text);
}
export function namedEngine(text){
  const direct=text.match(/(?:в|через|using|in|with|on|search|use)\s+(?:поисковик[еа]?\s+)?((?:google|yandex|гугл[еа]?|яндекс[еа]?)(?:\s*(?:и|and|[,/])\s*(?:в\s+)?(?:google|yandex|гугл[еа]?|яндекс[еа]?))?)/iu)?.[1]||'';
  const google=/google|гугл/iu.test(direct)||/погугли|загугли/iu.test(text);
  const yandex=/yandex|яндекс/iu.test(direct);
  return google&&yandex?'both':google?'google':yandex?'yandex':'auto';
}
