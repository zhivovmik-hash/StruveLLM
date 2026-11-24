"""Owned FFmpeg helpers: preserve original clip, condition continuation on its last frame."""
import hashlib,json,pathlib,subprocess,sys
job=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8-sig'))
directory=pathlib.Path(job['directory']);source=pathlib.Path(job['source']);ffmpeg=job['ffmpeg'];threads=str(job['cpu_threads'])
def run(args):
    p=subprocess.run([ffmpeg,'-hide_banner','-nostdin',*map(str,args)],capture_output=True)
    if p.returncode:raise RuntimeError(p.stderr.decode('utf-8','replace')[-3500:])
    return p
if job['stage']=='prepare':
    scan=run(['-threads',threads,'-i',source,'-map','0:v:0','-vf','fps=24','-progress','pipe:1','-f','null','-'])
    counts=[int(line.split('=',1)[1]) for line in scan.stdout.decode().splitlines() if line.startswith('frame=')]
    if not counts or counts[-1]<1:raise ValueError('Previous video has no decodable frames')
    count=counts[-1];target=int(job['target_frames'])
    if target<=count:raise ValueError('Requested total length must exceed the existing clip; this command extends video')
    if not 24<=target<=360:raise ValueError('Requested duration is outside 1–15 seconds')
    needed=target-count+1 # First generated frame duplicates the last original frame.
    native=5+max(0,needed-5+16)//17*17
    run(['-loglevel','error','-y','-threads',threads,'-i',source,'-vf',f"fps=24,select=eq(n\\,{count-1}),scale={job['width']}:{job['height']}",'-frames:v','1',directory/'last-frame.png'])
    info=dict(source=str(source),source_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),source_frames=count,target_frames=target,continuation_frames=native,source_audio='Audio:' in scan.stderr.decode('utf-8','replace'))
    (directory/'video-preparation.json').write_text(json.dumps(info,indent=2),encoding='utf-8')
    print(json.dumps(info))
elif job['stage']=='finish':
    info=json.loads((directory/'video-preparation.json').read_text())
    if hashlib.sha256(source.read_bytes()).hexdigest()!=info['source_sha256']:raise ValueError('Source video changed during generation')
    filters=f"[0:v]fps=24,scale={job['width']}:{job['height']},setsar=1,setpts=PTS-STARTPTS[v0];[1:v]fps=24,trim=start_frame=1,setsar=1,setpts=PTS-STARTPTS[v1];"
    audio=info['source_audio'] and job.get('generated_audio')
    if audio:
        filters+='[0:a]aformat=sample_rates=32000:channel_layouts=stereo,asetpts=PTS-STARTPTS[a0];[1:a]atrim=start=0.041666667,aformat=sample_rates=32000:channel_layouts=stereo,asetpts=PTS-STARTPTS[a1];[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]'
        maps=['-map','[v]','-map','[a]','-c:a','aac']
    else:
        filters+='[v0][v1]concat=n=2:v=1:a=0[v]';maps=['-map','[v]','-an']
    run(['-loglevel','error','-y','-i',source,'-i',directory/'render.avi','-filter_complex_threads',threads,'-filter_complex',filters,*maps,'-threads',threads,'-c:v','libx264','-crf','18','-pix_fmt','yuv420p','-frames:v',str(info['target_frames']),'-t',str(info['target_frames']/24),'-movflags','+faststart',directory/'video.mp4'])
else:raise ValueError('Unsupported video edit stage')
