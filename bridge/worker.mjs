// Explicit diagnostic process: no model, no training, no GPU work.
const started=Date.now();
console.log(JSON.stringify({event:'started',pid:process.pid,cuda_devices:process.env.CUDA_VISIBLE_DEVICES,cpu_threads:process.env.OMP_NUM_THREADS,argv:process.argv.slice(2)}));
const interval=setInterval(()=>{console.log(JSON.stringify({event:'heartbeat',elapsed_seconds:(Date.now()-started)/1000}));if(Date.now()-started>60000){clearInterval(interval);process.exit(0)}},1000);
