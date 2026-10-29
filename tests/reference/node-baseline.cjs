'use strict';
// Harness for the unmodified MIT-declared 5-web-port server, test use only.
const { createServer } = require('./5-web-port-server.cjs');
(async()=>{
 const {server}=await createServer({mirrorDir:process.argv[2],cacheDir:process.argv[3]});
 server.listen(0,'127.0.0.1',()=>process.stdout.write(JSON.stringify({origin:`http://127.0.0.1:${server.address().port}`})+'\n'));
})().catch(error=>{console.error(error);process.exit(1);});
