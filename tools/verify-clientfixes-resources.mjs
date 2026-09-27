import fs from 'node:fs';
import crypto from 'node:crypto';
const dir='src-tauri/resources/client-fixes/';
const report=JSON.parse(fs.readFileSync(dir+'payload-report.json','utf8'));
for(const [file,key] of [['BravoHotelGame-ClientFixes_P.pak','pak_sha256'],['BravoHotelGame-ClientFixes_P.sig','sig_sha256']]) {
 const bytes=fs.readFileSync(dir+file);
 if(crypto.createHash('sha256').update(bytes).digest('hex')!==report[key]) throw new Error('Resource hash mismatch: '+file);
 console.log('Verified '+file+' ('+bytes.length+' bytes)');
}