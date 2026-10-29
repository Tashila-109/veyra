"""Assertions on retained window pixels; no application or desktop mutation."""
import hashlib, io, json, math, pathlib, statistics, subprocess
from PIL import Image, ImageCms, ImageDraw

def normalized(text):
    return ' '.join(text.split())

class Paint:
    wrap_left_height_fraction = .25

    def __init__(self, proof, name, state, bounds=None, ocr_upscale=2, control_dispatch=True):
        self.proof, self.name, self.state = pathlib.Path(proof), name, state
        self.control_dispatch = control_dispatch
        assert (self.proof/'lane').read_text().strip() == 'fake', 'paint assertions require the owned fake lane'
        self.path = self.proof/(name+'.png')
        source_image = Image.open(self.path)
        self.profile = bool(source_image.info.get('icc_profile'))
        self.image = ImageCms.profileToProfile(source_image,
            ImageCms.ImageCmsProfile(io.BytesIO(source_image.info['icc_profile'])),
            ImageCms.createProfile('sRGB'),outputMode='RGB') if self.profile else source_image.convert('RGB')
        if bounds is None:
            command = [str(self.proof/'winid'), str(state['pid']), '--bounds']
            (self.proof/(name+'-bounds-invocation.json')).write_text(json.dumps(command)+'\n')
            result = subprocess.run(command, capture_output=True, text=True)
            (self.proof/(name+'-bounds.stderr')).write_text(result.stderr)
            assert result.returncode == 0, 'owned-window bounds failed'
            bounds = [float(value) for value in result.stdout.strip().split(',')]
        self.scale = self.image.width/bounds[2]
        assert abs(self.image.height-self.scale*bounds[3]) <= 1, (self.image.size,bounds)
        self.width, self.height = bounds[2:]
        self.checks = []
        binary = self.proof/'read-painted-text'
        source = pathlib.Path(__file__).with_suffix('.swift')
        digest = hashlib.sha256(source.read_bytes()).hexdigest()
        marker = self.proof/'read-painted-text.sha256'
        if not binary.exists() or not marker.exists() or marker.read_text() != digest:
            command = ['swiftc','-O',str(source),'-o',str(binary)]
            (self.proof/'read-painted-text-invocation.json').write_text(json.dumps(command)+'\n')
            result = subprocess.run(command, capture_output=True, text=True)
            (self.proof/'read-painted-text-build.txt').write_text(result.stdout+result.stderr)
            assert result.returncode == 0, 'painted-text helper did not compile'
            marker.write_text(digest)
        assert ocr_upscale in [1,2,3], ('unsupported OCR upscale',ocr_upscale)
        self.ocr_upscale = ocr_upscale
        ocr_path = self.path
        if ocr_upscale!=1:
            ocr_path = self.proof/(name+'-ocr-input.png')
            self.image.resize((self.image.width*ocr_upscale,self.image.height*ocr_upscale),Image.Resampling.LANCZOS).save(ocr_path)
        command = [str(binary),str(ocr_path)]
        (self.proof/(name+'-ocr-invocation.json')).write_text(json.dumps(command)+'\n')
        result = subprocess.run(command,capture_output=True,text=True)
        (self.proof/(name+'-ocr.stderr')).write_text(result.stderr)
        assert result.returncode == 0, 'painted-text recognition failed'
        self.rows = json.loads(result.stdout)
        if ocr_upscale!=1:
            raw_name = name+'-ocr-source-rows.json'
            (self.proof/raw_name).write_text(json.dumps(self.rows,indent=2)+'\n')
            self.rows = [dict(row,box=[value/ocr_upscale for value in row['box']],
                words=[dict(word,box=[value/ocr_upscale for value in word['box']]) for word in row['words']]) for row in self.rows]
            self.checks.append({'ocr_preprocess':{'integer_upscale':ocr_upscale,'filter':'LANCZOS',
                'input':ocr_path.name,'input_sha256':hashlib.sha256(ocr_path.read_bytes()).hexdigest(),
                'source_rows':raw_name,'boxes':'mapped to captured pixels','confidence':'unchanged',
                'raw_color_pixels':'unchanged'}})
        (self.proof/(name+'-painted-text.json')).write_text(json.dumps(self.rows,indent=2)+'\n')

    def matches(self, text, region=None):
        text = normalized(text)
        found = []
        for row in self.rows:
            line = normalized(row['text'])
            if text not in line: continue
            start,end = line.index(text),line.index(text)+len(text)
            offset,boxes = 0,[]
            for word in row['words']:
                word_start = line.index(word['text'],offset)
                word_end = word_start+len(word['text'])
                if word_end>start and word_start<end: boxes.append(word['box'])
                offset = word_end
            assert boxes, ('recognized text has no word geometry',self.name,text,row)
            box = [min(b[0] for b in boxes),min(b[1] for b in boxes),max(b[2] for b in boxes),max(b[3] for b in boxes)]
            if region is None or self.inside(box,self.box(region)):
                found.append(dict(row,box=box))
        ordered = sorted((row for row in self.rows if region is None or self.inside(row['box'],self.box(region))),
            key=lambda row:(row['box'][1],row['box'][0]))
        for index,first in enumerate(ordered):
            joined,previous,tolerances = [first],first,[]
            phrase = normalized(first['text'])
            for row in ordered[index+1:]:
                box,before = row['box'],previous['box']
                gap = box[1]-before[3]
                line_height = max(box[3]-box[1],before[3]-before[1])
                tolerance = self.wrap_left_height_fraction*max(box[3]-box[1],first['box'][3]-first['box'][1])
                if (abs(box[0]-first['box'][0])>tolerance or box[1]<=before[1]
                    or gap<0 or gap>line_height): break
                joined.append(row)
                tolerances.append(tolerance/self.scale)
                phrase += ' '+normalized(row['text'])
                if phrase==text:
                    box = [min(item['box'][0] for item in joined),min(item['box'][1] for item in joined),
                        max(item['box'][2] for item in joined),max(item['box'][3] for item in joined)]
                    if region is None or self.inside(box,self.box(region)):
                        found.append({'text':phrase,'box':box,
                            'confidence':min(item['confidence'] for item in joined),
                            'joined_rows':list(joined),
                            'left_tolerance_height_fraction':self.wrap_left_height_fraction,
                            'left_tolerances_points':list(tolerances)})
                    break
                if not text.startswith(phrase+' '): break
                previous = row
        return found

    @staticmethod
    def inside(inner, outer):
        return inner[0]>=outer[0] and inner[1]>=outer[1] and inner[2]<=outer[2] and inner[3]<=outer[3]

    def box(self, rectangle):
        return tuple(round(value*self.scale) for value in rectangle)

    def text(self, text, region=None, count=None):
        controls = self.control_text(text,region) if getattr(self,'control_dispatch',False) else None
        rows = self.matches(text,region) if controls is None else controls
        assert rows and (count is None or len(rows)==count), ('painted text missing or repeated',self.name,text,rows)
        assert all(row['confidence'] >= .5 for row in rows), ('low-confidence painted text',self.name,text,rows)
        self.checks.append({'text':text,'region':region,'matches':rows})
        return rows[0]['box']

    def control_text(self, text, region):
        labels = self.state['look']['control_labels']
        chosen = []
        for name,label in labels.items():
            if normalized(label)!=normalized(text): continue
            assert name in self.state['look']['controls'], ('control caption has no bounds',self.name,name)
            x,y,width,height = self.state['look']['controls'][name]
            if region is not None and (x+width<=region[0] or y+height<=region[1] or x>=region[2] or y>=region[3]): continue
            radius = self.state['look']['layout']['control_radius']
            chosen.append((name,(max(0,x+radius),max(0,y+radius),min(self.width,x+width-radius),min(self.height,y+height-radius))))
        if not chosen: return None
        found = []
        def translated(row,offset):
            result = dict(row)
            if 'box' in row:
                result['crop_box'] = row['box']
                result['box'] = [value+offset[index%2] for index,value in enumerate(row['box'])]
            for key in ['words','joined_rows']:
                if key in row: result[key] = [translated(value,offset) for value in row[key]]
            return result
        for name,rectangle in chosen:
            label = 'control-text-'+hashlib.sha256(name.encode()).hexdigest()[:16]
            paint = self.crop(label,rectangle,pad=True)
            matches = paint.matches(text)
            paint.checks.append({'control':name,'expected':text,'matches':matches,'sole_path':True})
            paint.finish()
            offset = paint.capture_offset
            mapped = [translated(row,offset) for row in matches]
            mapped = [row for row in mapped if region is None or self.inside(row['box'],self.box(region))]
            for row in mapped: row['source_crop'] = paint.name
            self.checks.append({'control_text':name,'text':text,'rectangle_points':rectangle,
                'capture_offset_pixels':offset,'matches':mapped,'receipt':paint.name+'-pixels.json'})
            found.extend(mapped)
        return found

    def absent(self, text, region=None):
        rows = self.matches(text,region)
        assert not rows, ('unexpected painted text',self.name,text,rows)
        self.checks.append({'absent':text,'region':region})

    def before(self, upper, lower):
        a,b = self.text(upper),self.text(lower)
        assert a[3] <= b[1], ('painted order',self.name,upper,lower,a,b)
        self.checks.append({'before':[upper,lower]})

    def color(self, rectangle, role, minimum=1, over=None, background_only=False):
        expected = self.state['look']['colors'][role]
        if len(expected)==4 and expected[3]!=255:
            assert over is not None, ('translucent color needs its opaque underlay',role)
            background = self.state['look']['colors'][over]
            assert len(background)==3 or background[3]==255, ('underlay is not opaque',over)
            alpha = expected[3]/255
            expected = [round(front*alpha+back*(1-alpha)) for front,back in zip(expected[:3],background[:3])]
        rectangle = self.box(rectangle)
        glyphs = []
        if background_only:
            rows = list(self.rows)
            rows += [row for check in self.checks for row in check.get('matches',[])]
            glyphs = [tuple([math.floor(word['box'][0])-1,math.floor(word['box'][1])-1,
                math.ceil(word['box'][2])+1,math.ceil(word['box'][3])+1])
                for row in rows for word in row.get('words',[])]
        image = self.image.crop(rectangle)
        if background_only:
            mask = Image.new('1',image.size,1)
            draw = ImageDraw.Draw(mask)
            for left,top,right,bottom in glyphs:
                draw.rectangle((left-rectangle[0],top-rectangle[1],right-rectangle[0]-1,bottom-rectangle[1]-1),fill=0)
            count = sum(allowed and max(abs(a-b) for a,b in zip(pixel,expected))<=5
                for pixel,allowed in zip(image.getdata(),mask.getdata()))
        else:
            count = sum(max(abs(a-b) for a,b in zip(pixel,expected))<=5 for pixel in image.getdata())
        assert count >= minimum*self.scale*self.scale, ('painted color missing',self.name,role,rectangle,count)
        self.checks.append({'color':role,'over':over,'rectangle':rectangle,'pixels':count,
            'background_only':background_only,'excluded_glyph_boxes':glyphs,
            'minimum_pixels':minimum*self.scale*self.scale})

    def text_color(self, text, role):
        box = self.text(text)
        self.color(tuple(value/self.scale for value in box),role,minimum=1)
        pixels = list(self.image.crop(tuple(round(value) for value in box)).getdata())
        background = [statistics.median(pixel[channel] for pixel in pixels) for channel in range(3)]
        contrast = lambda pixel:max(abs(a-b) for a,b in zip(pixel,background))
        peak = max(contrast(pixel) for pixel in pixels)
        expected = self.state['look']['colors'][role][:3]
        count = sum(contrast(pixel)>=peak*.9 and max(abs(a-b) for a,b in zip(pixel,expected))<=5 for pixel in pixels)
        assert count>=self.scale*self.scale, ('painted text peak has wrong color',self.name,role,count,peak)
        self.checks.append({'text_peak_color':role,'text':text,'background':background,'peak_contrast':peak,'pixels':count})

    def plate(self, text, role, over=None, region=None):
        box = self.text(text,region)
        left,top,right,bottom = (value/self.scale for value in box)
        pad = self.state['look']['layout']['small_icon']/4
        self.color((left-pad,top-pad,right+pad,bottom+pad),role,minimum=pad,over=over,background_only=True)

    def control(self, name):
        key = 'Name("'+name+'")'
        assert key in self.state['look']['controls'], ('painted control bounds missing',self.name,name)
        x,y,width,height = self.state['look']['controls'][key]
        return x,y,x+width,y+height

    def outline(self, name):
        left,top,right,bottom = self.control(name)
        radius = self.state['look']['layout']['control_radius']
        border = self.state['look']['layout']['border']
        self.color((left+radius,top,right-radius,top+border),'border',
            minimum=(right-left-radius*2)*border*.8,over='card')

    def filled_keycap(self, name, caption, variant):
        left,top,right,bottom = self.control(name)
        label = self.text(caption,(left,top,right,bottom),count=1)
        radius = self.state['look']['layout']['control_radius']
        rectangle = (label[2]/self.scale+1,top+radius,right-radius,bottom-radius)
        assert rectangle[0]<rectangle[2], ('filled keycap has no trailing region',self.name,name,rectangle)
        assert variant in ['primary','danger'], ('unknown filled button variant',variant)
        role = 'button_'+variant+'_foreground'
        ink = self.state['look']['colors'][role][:3]
        pixels = self.box(rectangle)
        sample = self.image.crop(pixels)
        fill = max(sample.getcolors(sample.width*sample.height),key=lambda item:item[0])[1]
        direction = [a-b for a,b in zip(ink,fill)]
        energy = sum(value*value for value in direction)
        assert energy>0, ('filled keycap has no foreground contrast',self.name,name)
        accepted = []
        for y in range(pixels[1],pixels[3]):
            for x in range(pixels[0],pixels[2]):
                pixel = self.image.getpixel((x,y))
                coverage = max(0,min(1,sum((p-b)*d for p,b,d in zip(pixel,fill,direction))/energy))
                residual = max(abs(p-(b+coverage*d)) for p,b,d in zip(pixel,fill,direction))
                if coverage>=.5 and residual<=5:
                    accepted.append({'pixel':[x,y],'coverage':coverage,'residual':residual})
        receipt = {'filled_keycap_foreground':name,'caption':caption,'rectangle':rectangle,
            'role':role,'variant':variant,'foreground':ink,'observed_fill':fill,
            'minimum_coverage':.5,'maximum_channel_error':5,'samples':accepted}
        self.checks.append(receipt)
        assert len(accepted)>=self.scale*self.scale, ('painted filled keycap foreground missing',self.name,receipt)

    def wrapped_send(self):
        assert self.state['composer']['focused'], ('wrapped Send scene',self.name,self.width)
        assert self.state['composer']['width_override']==440, ('wrapped Send column',self.name)
        left,top,right,bottom = self.control('send')
        assert top>=self.control('model')[3], ('Send scene did not wrap',self.name)
        label = self.text('Send',(left,top,right,bottom),count=1)
        model = self.text('Default model')
        assert top>=model[3]/self.scale, ('Send scene did not wrap',self.name)
        card_x,card_y,card_width,card_height = self.state['composer']['surface_bounds']
        card_right = card_x+card_width
        input_x,_,input_width,_ = self.state['composer']['input_bounds']
        padding = card_right-input_x-input_width
        border = self.state['look']['layout']['border']
        middle = (top+bottom)/2
        band = self.box((card_x,middle-border,card_right+border,middle+border))
        solid = self.state['look']['colors']['solid'][:3]
        accent = self.state['look']['colors']['accent'][:3]
        underlay = self.state['look']['colors']['card'][:3]
        direction = [a-b for a,b in zip(accent,underlay)]
        energy = sum(value*value for value in direction)
        assert energy>0, ('card edge role has no contrast',self.name)
        button_pixels,card_pixels,card_matches = [],[],[]
        for y in range(band[1],band[3]):
            for x in range(band[0],band[2]):
                pixel = self.image.getpixel((x,y))
                if max(abs(a-b) for a,b in zip(pixel,solid))<=5: button_pixels.append((x,y))
                if abs((x+1)/self.scale-card_right)<=border*2:
                    coverage = max(0,min(1,sum((p-b)*d for p,b,d in zip(pixel,underlay,direction))/energy))
                    residual = max(abs(p-(b+coverage*d)) for p,b,d in zip(pixel,underlay,direction))
                    if coverage>=.5 and residual<=5:
                        card_pixels.append((x,y))
                        card_matches.append({'pixel':[x,y],'coverage':coverage,'residual':residual})
        edge_probe = {'band':band,'control':[left,top,right,bottom],'card_right':card_right,
            'solid':solid,'accent':accent,'solid_pixels':len(button_pixels),'accent_pixels':len(card_pixels),
            'card_samples':[(x,self.image.getpixel((x,(band[1]+band[3])//2)))
                for x in range(round((card_right-border*3)*self.scale),round((card_right+border*2)*self.scale))]}
        assert button_pixels and card_pixels, ('wrapped Send paint edges missing',self.name,edge_probe)
        button_edge = (max(x for x,y in button_pixels)+1)/self.scale+border
        card_edge = (max(x for x,y in card_pixels)+1)/self.scale
        receipt = {'painted_send_right':button_edge,'painted_card_right':card_edge,
            'expected_inset':padding,'tolerance_points':border,'solid_pixels':len(button_pixels),
            'accent_pixels':len(card_pixels),'caption_box':label,'wrapped':True,
            'card_edge_role':{'underlay':underlay,'foreground':accent,'minimum_coverage':.5,'maximum_channel_error':5,'samples':card_matches}}
        self.checks.append({'wrapped_send':receipt})
        assert abs(card_edge-button_edge-padding)<=border, ('painted Send is not trailing',self.name,receipt)

    def crop(self, label, rectangle, pad=False):
        if not hasattr(self,'crops'): self.crops = {}
        rectangle = tuple(rectangle)
        if label in self.crops:
            previous,paint = self.crops[label]
            assert previous==(rectangle,pad), ('OCR crop geometry changed',self.name,label)
            return paint
        left,top,right,bottom = rectangle
        assert 0<=left<right<=self.width and 0<=top<bottom<=self.height, ('OCR crop outside capture',self.name,label,rectangle)
        pixels = self.box(rectangle)
        name = self.name+'-'+label
        cropped = self.image.crop(pixels)
        padding = None
        origin = (0,0)
        if pad:
            background = max(cropped.getcolors(cropped.width*cropped.height),key=lambda item:item[0])[1]
            side = max(cropped.size)+cropped.height*2
            origin = ((side-cropped.width)//2,(side-cropped.height)//2)
            padded = Image.new('RGB',(side,side),background)
            padded.paste(cropped,origin)
            padding = {'background':background,'source_size':cropped.size,'origin_pixels':origin,
                'padded_size':padded.size,'upscaled':False}
            cropped = padded
        cropped.save(self.proof/(name+'.png'),icc_profile=self.image.info.get('icc_profile'))
        ocr_upscale = math.ceil(3/self.scale)
        paint = Paint(self.proof,name,self.state,[0,0,cropped.width/self.scale,cropped.height/self.scale],
            ocr_upscale=ocr_upscale,control_dispatch=False)
        paint.capture_offset = (pixels[0]-origin[0],pixels[1]-origin[1])
        self.crops[label] = (rectangle,pad),paint
        self.checks.append({'ocr_crop':name,'rectangle_points':rectangle,'rectangle_pixels':pixels,
            'backing_scale':self.scale,'receipt':name+'-pixels.json','recognition':'one pass, no fallback',
            'padding':padding,'ocr_upscale':ocr_upscale})
        return paint

    def header(self):
        state = self.state
        sidebar = state['sidebar']['width']
        header = state['look']['layout']['header']
        selected = state['selected_thread']
        if state['view'] in ['settings','new_thread']:
            expected = 'Settings' if state['view']=='settings' else 'New thread'
            assert state['look']['header_title']==expected, ('header title decision',self.name,expected)
            paint = self.crop('header-text',self.control('header-title'),pad=True)
            paint.text(expected)
            paint.finish()
            if state['header_branch']: raise AssertionError('branch leaked into a non-chat header')
        elif selected:
            words = selected['header_status'].split(' · ')[0]
            role = {'Working':'accent','NeedsYou':'warning','Done':'success','Failed':'danger'}.get(selected['status'],'text_muted')
            left,top,right,bottom = self.control('header-status')
            glyph = state['transcript']['layout']['status_glyph']
            assert glyph>0 and left+glyph<right, ('invalid header mark geometry',self.name,glyph)
            cut = math.ceil((left+glyph)*self.scale)
            paint = self.crop('header-text',(cut/self.scale,top,right,bottom),pad=True)
            mark_cut = {'group_bounds_points':[left,top,right,bottom],'status_glyph_points':glyph,
                'left_cut_pixels':cut,'backing_scale':self.scale,'rounding':'ceil',
                'sources':['look.controls.Name("header-status")','transcript.layout.status_glyph']}
            paint.checks.append({'status_mark_cut':mark_cut})
            (self.proof/(paint.name+'-status-cut.json')).write_text(json.dumps(mark_cut,indent=2)+'\n')
            box = paint.text(words)
            paint.color(tuple(value/paint.scale for value in box),role)
            paint.finish()
        scrolled = state['view'] not in ['settings','new_thread'] and state['transcript']['scroll_offset']<0
        self.color((sidebar+header,header-1,self.width-header,header),
            'border' if scrolled else 'bg',minimum=max(1,self.width-sidebar-header*2)*.9,
            over='bg' if scrolled else None)

    def transcript_text(self):
        state = self.state
        viewport = state['look']['viewport'][0]
        sidebar = state['sidebar']['width']
        layout = state['transcript']['layout']
        width = min(layout['column_max'],viewport-sidebar-layout['gutter']*2)
        left = sidebar+(viewport-sidebar-width)/2
        return self.crop('transcript-text',(left,state['look']['layout']['header'],
            left+width,state['composer']['surface_bounds'][1]))

    def composer(self):
        state = self.state
        if state['view']=='settings': return
        selected = state['selected_thread']
        provider = {'claude':'Claude Code','codex':'Codex','grok':'Grok'}[state['composer']['provider']]
        expected = ('Describe the task' if state['view']=='new_thread' else
            'Write now, it waits for Continue' if selected['hold'] is not None else
            'Queue a follow-up' if selected['status'] in ['Working','NeedsYou'] else 'Reply to '+provider)
        assert state['composer']['placeholder']==expected, ('placeholder decision',self.name)
        self.checks.append({'dump_placeholder':expected,'composer_focused':state['composer']['focused'],
            'control_focus':state['composer']['control_focus']})
        x,y,width,height = state['composer']['input_bounds']
        assert x>=0 and y>=0 and x+width<=self.width and y+height<=self.height, ('painted editor escaped window',self.name)
        if state['composer_text']=='' and not state['composer']['focused']:
            self.text(state['composer']['placeholder'],(x,y-1,x+width,y+height+1))
        if state['composer']['focused']:
            left,top,width,height = state['composer']['surface_bounds']
            radius = state['look']['layout']['composer_radius']
            border = state['look']['layout']['border']
            if state['window_active']:
                self.color((left,top+radius,left+border,top+height-radius),'accent',minimum=(height-radius*2)*.8)
            else:
                self.color((left,top+radius,left+border,top+height-radius),'border',minimum=(height-radius*2)*.8,over='card')
        if state['composer']['effort'] is None:
            self.text('Provider default',(state['sidebar']['width'],y,self.width,self.height))
        self.absent('Focus to use shortcuts')

    def transcript(self):
        state = self.state
        if state['view'] in ['new_thread','settings']: return
        if state['transcript']['scroll_offset']==0 and (state['transcript']['items'] or state['transcript']['family_card']):
            layout = state['transcript']['layout']
            sidebar = state['sidebar']['width']
            width = min(layout['column_max'],self.width-sidebar-layout['gutter']*2)
            left = sidebar+(self.width-sidebar-width)/2
            top = state['look']['layout']['header']+state['look']['layout']['border']
            bottom = state['look']['layout']['header']+state['look']['layout']['read_line']-state['look']['layout']['border']
            self.color((left,top,left+width,bottom),'bg',minimum=width*(bottom-top)*.98)

    def approval_inset(self):
        dock = self.state['composer']['dock']
        left,top,width,height = dock['bounds']
        inset = dock['content_inset']
        ring = dock['focus_inset']+dock['focus_border']
        radius = self.state['look']['layout']['control_radius']
        assert inset>ring and width>inset*2 and height>radius*2, ('invalid approval inset geometry',self.name,dock['bounds'],inset,ring)
        expected = self.state['look']['colors']['card'][:3]
        checks = []
        for side,rectangle in [('left',(left+ring,top+radius,left+inset,top+height-radius)),
                ('right',(left+width-inset,top+radius,left+width-ring,top+height-radius))]:
            box = self.box(rectangle)
            assert box[0]<box[2] and box[1]<box[3], ('empty approval inset band',self.name,side,box)
            unexpected = []
            for y in range(box[1],box[3]):
                for x in range(box[0],box[2]):
                    pixel = self.image.getpixel((x,y))
                    if max(abs(a-b) for a,b in zip(pixel,expected))>5:
                        unexpected.append({'xy':[x,y],'rgb':pixel})
            record = {'side':side,'rectangle_points':rectangle,'rectangle_pixels':box,
                'expected_card':expected,'maximum_channel_error':5,'unexpected':unexpected}
            receipt = self.proof/(self.name+'-approval-'+side+'-inset.json')
            receipt.write_text(json.dumps(record,indent=2)+'\n')
            assert not unexpected, ('painted approval inset occupied',self.name,side,len(unexpected),receipt.name)
            checks.append({'side':side,'rectangle_points':rectangle,'receipt':receipt.name,
                'pixels':(box[2]-box[0])*(box[3]-box[1])})
        self.checks.append({'approval_inset':checks,'source':'measured dock frame and renderer padding/ring geometry'})

    def approval(self):
        dock = self.state['composer']['dock']
        assert dock is not None
        self.approval_inset()
        target = dock['request']['kind']['Approval']['action']['target']
        preview = target['preview']
        try: residual = json.loads(preview['text'])
        except (ValueError,TypeError): residual = None
        if isinstance(residual,dict) and not preview['truncated']:
            assert not (dock['preview'] or '').lstrip().startswith('{'), 'complete object did not use the names-and-values layout'
        self.text('Deny')
        self.outline('deny-'+dock['request']['id'])
        if dock['allow_once_enabled']:
            self.text('Allow once')
            self.color(self.control('allow-once-'+dock['request']['id']),'solid',minimum=10,background_only=True)
            if dock['focused']: self.filled_keycap('allow-once-'+dock['request']['id'],'Allow once','primary')
        else: self.absent('Allow once')
        if not dock['allow_in_thread_enabled']: self.absent('in this thread')
        else:
            self.outline('allow-thread-'+dock['request']['id'])
            self.color(self.control('allow-thread-'+dock['request']['id']),'warning',minimum=2)
        if target['kind']=='command':
            self.text('Run command')
            if self.name.startswith('approval-short-command'):
                assert target['command']=='echo ok' and dock['action_text']=='echo ok'
                top = self.text('Run command')[3]/self.scale
                bottom = self.text('Deny')[1]/self.scale
                region = (self.state['sidebar']['width'],top,self.width,bottom)
                self.text('$ echo ok',region,count=1)
                self.absent('`',region)
                self.absent('{',region)
                self.absent('}',region)
                self.plate('$ echo ok','code_wash',over='card',region=region)
        elif target['kind']=='file':
            outside = target['location']!='inside_project'
            tag = 'Read file' if target['operation']=='read' else ('Write outside the project' if outside else ('Write file' if target['operation']=='write' else 'Edit file'))
            self.text(tag)
            if outside: self.text_color(tag,'warning')
        else: self.text('Use tool')
        if self.name.startswith('codex-mcp-first'):
            assert residual['_meta']['extra']=='material'
            assert '_meta · extra: material' in dock['preview']
            self.text('extra: material')

    def finish(self):
        assert self.checks, 'empty painted assertion receipt'
        (self.proof/(self.name+'-pixels.json')).write_text(json.dumps({
            'image_sha256':hashlib.sha256(self.path.read_bytes()).hexdigest(),
            'backing_scale':self.scale,'converted_embedded_profile_to_srgb':self.profile,
            'ocr_upscale':getattr(self,'ocr_upscale',None),
            'checks':self.checks},indent=2)+'\n')
        return self

def capture_checked(proof, name, state):
    paint = Paint(proof,name,state)
    sidebar,header = state['sidebar']['width'],state['look']['layout']['header']
    drawer = state['drawer'] or {}
    if state['quit_sheet']['open']:
        paint.text('Quitting stops')
        cancel = paint.text('Cancel')
        paint.plate('Quit','danger',region=(cancel[2]/paint.scale,cancel[1]/paint.scale-8,
            paint.width,cancel[3]/paint.scale+8))
        paint.filled_keycap('confirm-quit','Quit','danger')
    elif drawer.get('close_sheet') is not None:
        cancel = paint.text('Cancel')
        paint.text('Archive thread?' if 'archive' in name else 'Close tab?')
        paint.plate('Archive' if 'archive' in name else 'Close tab','danger',
            region=(cancel[2]/paint.scale,cancel[1]/paint.scale-8,paint.width,cancel[3]/paint.scale+8))
        paint.filled_keycap('confirm-close-shell','Archive' if 'archive' in name else 'Close tab','danger')
    else:
        paint.header()
        paint.composer()
        paint.transcript()
        if name=='grok-toolbar-wrapped': paint.wrapped_send()
        if name=='grok-tool-title-expanded':
            tool = next(row for row in state['transcript']['items'] if row['kind']=='tool')
            assert tool['expanded'] and tool['body']['Tool']['call']['input_summary'] is None
            assert tool['body']['Tool']['call']['arguments']=={'variant':'Bash','command':'pwd'}
            region = (sidebar,header,paint.width,state['composer']['surface_bounds'][1])
            paint.text('command: pwd',region,count=1)
            paint.text('variant: Bash',region,count=1)
            paint.absent('$ pwd',region)
        if name.startswith('chat-short-command-expanded'):
            tool = next(row for row in state['transcript']['items'] if row['kind']=='tool')
            assert tool['body']['Tool']['call']['input_summary']=='echo ok'
            region = (sidebar,header,paint.width,state['composer']['input_bounds'][1])
            paint.text('$ echo ok',region,count=1)
            paint.plate('$ echo ok','code_wash',over='bg',region=region)
            for text in ['`','{','}','plaintext','bash']:
                paint.absent(text,region)
        dock = state['composer']['dock']
        if dock and 'Approval' in dock['request']['kind']: paint.approval()
        if dock and 'Question' in dock['request']['kind']:
            provider = {'claude':'Claude Code','codex':'Codex','grok':'Grok'}[state['composer']['provider']]
            paint.text(provider+' asks')
        if state['view']=='failed_thread':
            paint.text('Retry')
            paint.text('Open shell')
            paint.plate('Retry','danger_wash')
        family = state['transcript']['family_card']
        if family is not None:
            lane = paint.transcript_text()
            title = lane.text(family['title'])
            if name in ['multi-family-done','codex-family-done']:
                assert any((row['outcome'] or '').startswith('Done in') for row in state['transcript']['items']), ('family binding outcome missing from dump',name)
                lane.text('Done in')
            outcomes = lane.matches('Done in')
            if outcomes:
                lane.text('Done in')
                assert title[3] <= outcomes[0]['box'][1], ('family card follows its outcome',name,title,outcomes)
            lane.finish()
        if state['view']=='settings':
            control = paint.text('Ask first',(sidebar,header,paint.width,paint.height))
            label = paint.text('Default mode for new threads',(sidebar,header,control[0]/paint.scale,paint.height))
            paint.text('Accept edits',(control[0]/paint.scale,header,paint.width,paint.height))
            paint.text('Full access',(control[0]/paint.scale,header,paint.width,paint.height))
            paint.text('Window background')
            left,top,_,bottom = (value/paint.scale for value in label)
            paint.color((left-4,top,left-2,bottom),'card',minimum=bottom-top)
            description = {
                'ask_first':'Asks before it edits files. Asks before commands the CLI does not treat as safe. Commands can use the network.',
                'accept_edits':'Edits files without asking. Asks before commands the CLI does not treat as safe. Commands can use the network.',
                'full_access':'Runs every edit and command without asking, inside and outside the project folder, with no sandbox.',
            }[state['settings']['default_mode']]
            paint.text(description+' Applies to threads you create from now on.',
                (sidebar,header,paint.width,paint.height),count=1)
        if state['view']=='new_thread' and not state['composer']['send_enabled']:
            paint.text('Check again')
            paint.plate('Check again','warning_wash')
            paint.text_color('Send','text_muted')
            if state['composer']['disabled_reason'].startswith('Sign in with '):
                command = {'claude':'claude auth login','codex':'codex login','grok':'grok login'}[state['composer']['provider']]
                assert command in state['composer']['disabled_reason']
                x,y,width,height = state['composer']['input_bounds']
                check = paint.text('Check again')
                paint.color((x,y+height,check[0]/paint.scale,check[3]/paint.scale),
                    'code_wash',over='warning_wash',minimum=state['look']['layout']['small_icon'])
        if drawer.get('open') and any(tab['exited'] for tab in drawer['tabs']):
            active = next(tab for tab in drawer['tabs'] if tab['number']==drawer['active_tab'])
            if active['exited']: paint.text('Restart')
    return paint.finish()

def sidebar_dims(proof, before, after, state):
    a,b = Paint(proof,before,state),Paint(proof,after,state)
    assert a.image.size==b.image.size and a.scale==b.scale, 'sheet capture rectangle changed'
    region = a.box((0,state['look']['layout']['header']*2,state['sidebar']['width'],a.height))
    colors = [state['look']['colors'][role][:3] for role in ['text','text_muted','icon']]
    pairs = list(zip(a.image.crop(region).getdata(),b.image.crop(region).getdata()))
    foreground = [(left,right) for left,right in pairs if any(
        max(abs(value-wanted) for value,wanted in zip(left,color))<=5 for color in colors)]
    changed = sum(max(abs(x-y) for x,y in zip(left,right))>20 for left,right in foreground)
    assert len(foreground)>10 and changed/len(foreground)>.8, ('sidebar foreground did not dim',before,after,len(foreground),changed)
    (pathlib.Path(proof)/(after+'-sidebar-dimming.json')).write_text(json.dumps({
        'before':before,'after':after,'region':region,'foreground_pixels':len(foreground),
        'dimmed_pixels':changed,'backing_scale':a.scale},indent=2)+'\n')
    sidebar_title_dims(a,b,state,before,after)

def sidebar_title_dims(a, b, state, before, after):
    title = state['selected_thread']['title']
    region = (0,state['look']['layout']['header']*2,state['sidebar']['width'],a.height)
    box = a.text(title,region,count=1)
    matches = a.checks[-1]['matches']
    rectangle = tuple(round(value) for value in box)
    colors = [state['look']['colors'][role][:3] for role in ['text','text_muted','icon']]
    pairs = list(zip(a.image.crop(rectangle).getdata(),b.image.crop(rectangle).getdata()))
    foreground = [(left,right) for left,right in pairs if any(
        max(abs(value-wanted) for value,wanted in zip(left,color))<=5 for color in colors)]
    changed = sum(max(abs(x-y) for x,y in zip(left,right))>20 for left,right in foreground)
    receipt = {'before':before,'after':after,'title':title,'rectangle_pixels':rectangle,
        'backing_scale':a.scale,'matches':matches,'foreground_pixels':len(foreground),
        'dimmed_pixels':changed,'minimum_changed_fraction':.8,'minimum_channel_change':20}
    (a.proof/(after+'-sidebar-title-dimming.json')).write_text(json.dumps(receipt,indent=2)+'\n')
    assert len(foreground)>10 and changed/len(foreground)>.8, (
        'sidebar title did not dim',before,after,title,len(foreground),changed)

def option_cursor_paints(proof, before, after, state, labels):
    a,b = Paint(proof,before,state),Paint(proof,after,state)
    assert a.image.size==b.image.size and a.scale==b.scale, 'cursor capture rectangle changed'
    x,y,width,_ = state['composer']['input_bounds']
    region = (state['sidebar']['width'],state['look']['layout']['header'],a.width,y)
    boxes = [a.text(label,region) for label in labels]
    pad = state['look']['layout']['small_icon']/2
    rectangle = a.box((x,min(box[1] for box in boxes)/a.scale-pad,
        x+width,max(box[3] for box in boxes)/a.scale+pad))
    changed = sum(left!=right for left,right in zip(a.image.crop(rectangle).getdata(),b.image.crop(rectangle).getdata()))
    assert changed > 20*a.scale*a.scale, ('option cursor changed no row pixels',before,after,changed)
    (pathlib.Path(proof)/(after+'-cursor-pixels.json')).write_text(json.dumps({
        'before':before,'after':after,'region':rectangle,'changed_pixels':changed,
        'backing_scale':a.scale},indent=2)+'\n')
