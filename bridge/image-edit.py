"""Bounded CPU image edits. Paths come only from the controller, never model JSON."""
import hashlib,json,os,pathlib,sys
import re
import cv2
import numpy as np
import onnxruntime as ort
from PIL import Image

def read_image(path):
    with Image.open(path) as im:
        if im.width*im.height>1536*1536 or min(im.size)<64: raise ValueError('Unsupported edit dimensions')
        im.load()
        return np.array(im.convert('RGB'))

def background_target(plan):
    return plan.get('target','').strip().casefold() in {
        'background','the background','backdrop','the backdrop','background surface','фон',
    }

def background_mask(rgb, point, directory, threads):
    """Find a separable coloured backdrop from its border and the planner's positive point."""
    h,w=rgb.shape[:2]
    px,py=point
    x,y=min(w-1,int(px*w/1000)),min(h-1,int(py*h/1000))
    hsv=cv2.cvtColor(rgb,cv2.COLOR_RGB2HSV)
    hue,sat,value=hsv.transpose(2,0,1)
    rim=max(2,min(w,h)//80)
    border=np.ones((h,w),bool)
    border[rim:-rim,rim:-rim]=False
    colourful=(sat>=32)&(value>=25)
    hist=np.bincount(hue[border&colourful],minlength=180)
    # Hue wraps at red. Use votes over a band rather than a single bin.
    votes=sum(np.roll(hist,shift) for shift in range(-4,5))
    peak=int(np.argmax(votes))
    distance=np.abs(hue.astype(np.int16)-peak)
    distance=np.minimum(distance,180-distance)
    patch=np.s_[max(0,y-3):y+4,max(0,x-3):x+4]
    same=colourful&(distance<=10)
    corners=(same[:rim,:rim],same[:rim,-rim:],same[-rim:,:rim],same[-rim:,-rim:])
    strong_border=votes[peak]>=border.sum()*.60 and sum(c.mean()>.7 for c in corners)>=3
    if votes[peak]<border.sum()*.35 or (not strong_border and same[patch].mean()<.5):
        raise ValueError('Background point and frame border do not identify one coloured backdrop')
    plausible=(distance<=10)&(sat>=24)&(value>=12)
    _,labels=cv2.connectedComponents(plausible.astype(np.uint8))
    edge_ids=np.concatenate((labels[0],labels[-1],labels[:,0],labels[:,-1]))
    edge_counts=np.bincount(edge_ids)
    keep=np.flatnonzero(edge_counts>=max(3,min(w,h)//100))
    connected=plausible&np.isin(labels,keep[keep!=0])
    if (not strong_border and not np.any(connected[patch])) or not .04<=connected.mean()<=.96:
        raise ValueError('Could not separate the foreground from the backdrop')
    # Use high-confidence colour away from the transition as fixed background.
    # Graph cut resolves the boundary and shadows using both colour distributions.
    kernel=np.ones((3,3),np.uint8)
    core=cv2.erode((connected&colourful&(distance<=5)).astype(np.uint8),kernel,iterations=2)>0
    outside=cv2.dilate(connected.astype(np.uint8),kernel,iterations=3)==0
    if not np.any(core) or not np.any(outside):
        raise ValueError('Missing reliable samples on both sides of the background boundary')
    trimap=np.where(connected,cv2.GC_PR_BGD,cv2.GC_PR_FGD).astype(np.uint8)
    trimap[core]=cv2.GC_BGD
    trimap[outside]=cv2.GC_FGD
    cv2.setRNGSeed(0)
    cv2.grabCut(rgb,trimap,None,np.zeros((1,65),np.float64),np.zeros((1,65),np.float64),4,cv2.GC_INIT_WITH_MASK)
    selected=(trimap==cv2.GC_BGD)|(trimap==cv2.GC_PR_BGD)
    # Keep transition pixels on the protected side unchanged, including metal highlights.
    selected &= connected
    # Shadows and reflections on the casing can share the backdrop's hue. Ask
    # SAM for an independent silhouette, prompted from the solid foreground.
    clearance=cv2.distanceTransform(selected.astype(np.uint8),cv2.DIST_L2,5)
    protected=clearance<=1
    cols=protected.sum(axis=0);rows=protected.sum(axis=1)
    xs=np.flatnonzero(cols>=cols.max()*.22);ys=np.flatnonzero(rows>=rows.max()*.22)
    pad=max(2,int(.025*min(w,h)))
    box=np.rint(1000*np.clip([(xs[0]-pad)/w,(ys[0]-pad)/h,(xs[-1]+pad)/w,(ys[-1]+pad)/h],0,1)).astype(int)
    depths=cv2.distanceTransform(protected.astype(np.uint8),cv2.DIST_L2,5)
    yy,xx=np.unravel_index(np.argmax(depths),depths.shape)
    if xx in (0,w-1) or yy in (0,h-1):
        depths=cv2.distanceTransform(np.pad(protected.astype(np.uint8),1),cv2.DIST_L2,5)[1:-1,1:-1]
        yy,xx=np.unravel_index(np.argmax(depths),depths.shape)
    point=[round(1000*xx/w),round(1000*yy/h)]
    subject=segment_object(rgb,{'box':box.tolist(),'point':point},directory,threads)>0
    agreement=np.count_nonzero(subject&protected)/max(1,np.count_nonzero(subject|protected))
    if agreement<.78:
        raise ValueError('The foreground silhouette is ambiguous; a more specific selection is required')
    # Also protect the one-pixel transition outside the independent silhouette.
    veto=cv2.dilate(subject.astype(np.uint8),kernel,iterations=1)>0
    accepted=selected&~veto
    clearance=cv2.distanceTransform(accepted.astype(np.uint8),cv2.DIST_L2,5)
    alpha=np.clip((clearance-1)/1.5,0,1)
    # SAM predicts on a coarse grid. Resolve its few-pixel margin against
    # the source colours so the old tabletop does not form a stepped halo.
    near_edge=cv2.distanceTransform((~accepted).astype(np.uint8),cv2.DIST_L2,5)<=max(3,min(h,w)*.025)
    low_sat,high_sat=np.percentile(sat[core],[1,99])
    typical_sat=(sat>=low_sat-8)&(sat<=high_sat+8)
    bright_enough=value>=np.percentile(value[core],60)*.34
    edge_alpha=np.clip((8-distance)/4,0,1)
    recover=connected&near_edge&typical_sat&bright_enough
    return np.rint(255*np.maximum(alpha,edge_alpha*recover)).astype(np.uint8)

def segment(rgb,plan,directory,threads):
    if background_target(plan):
        return background_mask(rgb,plan['point'],directory,threads)
    return segment_object(rgb,plan,directory,threads)

def segment_object(rgb,plan,directory,threads):
    h,w=rgb.shape[:2]
    x0,y0,x1,y1=plan['box']; px,py=plan['point']
    if not(0<=x0<px<x1<=1000 and 0<=y0<py<y1<=1000): raise ValueError('Point must lie inside target box')
    left,top=int(x0*w/1000),int(y0*h/1000)
    right,bottom=min(w,int(x1*w/1000)+1),min(h,int(y1*h/1000)+1)
    x,y=min(w-1,int(px*w/1000)),min(h-1,int(py*h/1000))
    if (right-left)*(bottom-top)<25 or (right-left)*(bottom-top)>w*h*.85: raise ValueError('Target box is empty or too broad')
    options=ort.SessionOptions();options.intra_op_num_threads=threads;options.inter_op_num_threads=1
    encoder=ort.InferenceSession(str(pathlib.Path(directory)/'vision_encoder.onnx'),sess_options=options,providers=['CPUExecutionProvider'])
    decoder=ort.InferenceSession(str(pathlib.Path(directory)/'prompt_encoder_mask_decoder.onnx'),sess_options=options,providers=['CPUExecutionProvider'])
    scale=1024/max(h,w); nh,nw=int(h*scale+.5),int(w*scale+.5)
    resized=np.array(Image.fromarray(rgb).resize((nw,nh),Image.Resampling.BILINEAR)).astype(np.float32)/255
    normalized=(resized-np.array([.485,.456,.406],np.float32))/np.array([.229,.224,.225],np.float32)
    pixels=np.zeros((1,3,1024,1024),np.float32);pixels[0,:,:nh,:nw]=normalized.transpose(2,0,1)
    embeddings=encoder.run(None,{'pixel_values':pixels})
    # The point alone can select one painted stripe instead of its physical
    # object. Supply the vision model's object bounds as SAM box prompts too.
    scores,masks=decoder.run(None,{'input_points':np.array([[[[x*scale,y*scale],[left*scale,top*scale],[right*scale,bottom*scale]]]],np.float32),'input_labels':np.array([[[1,2,3]]],np.int64),'image_embeddings':embeddings[0],'image_positional_embeddings':embeddings[1]})
    bounds=np.zeros((h,w),bool);bounds[top:bottom,left:right]=True
    candidates=[]
    for index,low in enumerate(masks[0,0]):
        full=cv2.resize(low,(1024,1024),interpolation=cv2.INTER_LINEAR)[:nh,:nw]
        binary=(cv2.resize(full,(w,h),interpolation=cv2.INTER_LINEAR)>0).astype(np.uint8)
        area=int(binary.sum());outside=int((binary.astype(bool)&~bounds).sum())/max(1,area)
        score=float(scores[0,0,index])
        if binary[y,x] and area>=24 and outside<.15 and score>=.55:
            candidates.append((score-2*outside,binary))
    if not candidates: raise ValueError('Segmentation is uncertain; describe the target more precisely')
    binary=max(candidates,key=lambda item:item[0])[1]
    count,components=cv2.connectedComponents(binary)
    selected=components[y,x]
    if selected==0: raise ValueError('Could not locate requested object')
    binary=(components==selected).astype(np.uint8)
    contours,_=cv2.findContours(binary,cv2.RETR_EXTERNAL,cv2.CHAIN_APPROX_SIMPLE)
    cv2.drawContours(binary,contours,-1,1,-1)
    # The vision box is approximate. Keep the accepted SAM boundary (including
    # small spill beyond the box) instead of cutting off rims, feet or handles.
    pixels=int(binary.sum())
    if pixels<max(24,(right-left)*(bottom-top)*.04): raise ValueError('Object mask is too small; describe the target more precisely')
    # Feather inward only: every pixel outside the mask remains byte-identical.
    alpha=np.minimum(cv2.distanceTransform(binary,cv2.DIST_L2,3)/1.5,1)
    mask=np.rint(alpha*255).astype(np.uint8)
    return mask

def recolor(rgb,mask,color):
    if len(color)!=3 or any(type(v) is not int or not 0<=v<=255 for v in color): raise ValueError('Invalid target RGB color')
    source=rgb.astype(np.float32)
    lum=source@np.array([.2126,.7152,.0722],np.float32)
    lo,hi=np.percentile(lum[mask>0],[3,97])
    shade=np.clip((np.log1p(lum)-np.log1p(lo))/max(.1,np.log1p(hi)-np.log1p(lo)),0,1)
    target=np.array(color,np.float32)[None,None,:]*(.62+.32*shade[:,:,None])
    target+=(255-np.array(color,np.float32))[None,None,:]*(.14*shade[:,:,None]**6)
    # Retain local illumination hue (for example a warm lamp reflected on paint).
    target+=np.clip(source-lum[:,:,None],-45,45)*.45
    return np.clip(target,0,255).astype(np.uint8)

def composite(source,edited,mask):
    if source.shape!=edited.shape or mask.shape!=source.shape[:2]: raise ValueError('Edit geometry mismatch')
    a=mask.astype(np.float32)[:,:,None]/255
    output=np.rint(source*(1-a)+edited*a).clip(0,255).astype(np.uint8)
    output[mask==0]=source[mask==0]
    changed=np.any(output!=source,axis=2)
    if np.any(changed & (mask==0)): raise ValueError('Outside-mask pixels changed')
    if int(changed.sum())<16: raise ValueError('Editor produced no visible change')
    return output,{'changed_pixels':int(changed.sum()),'outside_mask_changed_pixels':0,'mask_pixels':int((mask>0).sum()),'total_pixels':int(mask.size)}

def backdrop_color_edit(source,edited,mask,prompt):
    """Retain the lighting and texture of the photographed surface on colour-only edits."""
    sentence=re.split(r'[.!;]',prompt.casefold(),maxsplit=1)[0].strip()
    colours={'red':0,'orange':15,'yellow':30,'green':60,'cyan':90,'blue':118,
             'purple':145,'violet':145,'magenta':150,'pink':170,'teal':88}
    match=re.fullmatch(r'(?:change|turn|make|set|recolor|recolour|paint) (?:the )?(?:background|backdrop)(?: colou?r)? (?:to |into )?(?:a )?(?:solid |plain |uniform )?('+'|'.join(colours)+r')',sentence)
    if match is None:
        return edited
    interior=cv2.erode((mask==255).astype(np.uint8),np.ones((3,3),np.uint8),iterations=2)>0
    if not interior.any():
        raise ValueError('Too little background to verify its new colour')
    hsv=cv2.cvtColor(edited,cv2.COLOR_RGB2HSV)
    delta=np.abs(hsv[:,:,0].astype(np.int16)-colours[match[1]])
    in_colour=interior&(np.minimum(delta,180-delta)<=22)&(hsv[:,:,1]>=48)&(hsv[:,:,2]>=32)
    if in_colour.sum()<.65*interior.sum():
        raise ValueError('The rendered backdrop does not match the requested colour')
    # The actual GPU render provides the new colour. Source luminance supplies
    # the original grain and cast shadows, which a whole-frame render can move.
    palette=np.median(edited[in_colour],axis=0)
    luminance=source.astype(np.float32)@np.array([.2126,.7152,.0722],np.float32)
    reference=max(1,np.percentile(luminance[interior],80))
    return np.rint(np.clip(palette*luminance[:,:,None]/reference,0,255)).astype(np.uint8)

def main():
    job=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8-sig'))
    cv2.setNumThreads(max(1,min(16,int(job.get('cpu_threads',1)))))
    directory=pathlib.Path(job['directory']); source_path=pathlib.Path(job['source'])
    source=read_image(source_path)
    metrics={'source_sha256':hashlib.sha256(source_path.read_bytes()).hexdigest(),'width':source.shape[1],'height':source.shape[0]}
    if job['stage']=='prepare':
        plan=job['plan']; mask=segment(source,plan,job['segmentation_directory'],max(1,min(16,int(job.get('cpu_threads',1)))))
        Image.fromarray(mask).save(directory/'mask.png')
        if plan['operation']=='recolor':
            output,changes=composite(source,recolor(source,mask,plan['color']),mask)
            metrics.update(changes); Image.fromarray(output).save(directory/'image.png')
        Image.fromarray(source).save(directory/'source.png')
    elif job['stage']=='composite':
        mask=np.array(Image.open(directory/'mask.png').convert('L'))
        edited=read_image(directory/'render.png')
        # Reject a visibly misplaced mask instead of reporting an edit when
        # the model's actual change was discarded by the composite.
        x0,y0,x1,y1=job['plan']['box'];h,w=mask.shape
        bounds=np.zeros(mask.shape,bool);bounds[int(y0*h/1000):min(h,int(y1*h/1000)+1),int(x0*w/1000):min(w,int(x1*w/1000)+1)]=True
        strong=(np.max(np.abs(edited.astype(np.int16)-source.astype(np.int16)),axis=2)>=48)&bounds
        inside=int((strong&(mask>0)).sum());outside=int((strong&(mask==0)).sum())
        if background_target(job['plan']):
            if inside<max(16,np.count_nonzero(mask)*.12):raise ValueError('The renderer produced too little change in the selected background')
            edited=backdrop_color_edit(source,edited,mask,job['plan']['prompt'])
        elif outside>64 and outside>inside:raise ValueError('The requested change falls outside the selected object mask; describe the whole object more precisely')
        output,changes=composite(source,edited,mask)
        metrics.update(changes); Image.fromarray(output).save(directory/'image.png')
    else: raise ValueError('Unknown image editing stage')
    if (directory/'image.png').exists(): metrics['output_sha256']=hashlib.sha256((directory/'image.png').read_bytes()).hexdigest()
    (directory/'edit-metrics.json').write_text(json.dumps(metrics,indent=2),encoding='utf-8')
    print(json.dumps(metrics))

if __name__=='__main__': main()
