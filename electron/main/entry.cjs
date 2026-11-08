'use strict';
// Utility launchers run without loading the desktop, claiming its instance, or opening any book.
const agentMode=process.argv.findIndex(a=>a==='--ai-mcp' || a==='--ai-api');
if(agentMode>=0) require('./ai-mcp.cjs').start(process.argv[agentMode+1],process.argv[agentMode]==='--ai-api');
else require('./main.cjs');
