use eframe::egui::{self, Color32};

pub type Runs=Vec<(String,bool)>;
fn push(runs:&mut Runs,ch:char,changed:bool){
    if let Some((text,flag))=runs.last_mut(){if *flag==changed{text.push(ch);return;}}
    runs.push((ch.to_string(),changed));
}
// Trim common edges, then align Unicode characters. Bound worst-case memory/work.
pub fn diff(a:&str,b:&str)->(Runs,Runs,bool){
    let a:Vec<char>=a.chars().collect();let b:Vec<char>=b.chars().collect();
    let prefix=a.iter().zip(&b).take_while(|(a,b)|a==b).count();
    let suffix=a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(a,b)|a==b).count();
    let x=&a[prefix..a.len()-suffix];let y=&b[prefix..b.len()-suffix];
    let mut left=Vec::new();let mut right=Vec::new();
    for ch in &a[..prefix]{push(&mut left,*ch,false);push(&mut right,*ch,false);}
    let coarse=(x.len()+1).saturating_mul(y.len()+1)>16_000_000 || x.len().max(y.len())>u16::MAX as usize;
    if coarse {for ch in x{push(&mut left,*ch,true);}for ch in y{push(&mut right,*ch,true);}}
    else {
        let width=y.len()+1;let mut table=vec![0u16;(x.len()+1)*width];
        for i in (0..x.len()).rev(){for j in (0..y.len()).rev(){table[i*width+j]=if x[i]==y[j]{1+table[(i+1)*width+j+1]}else{table[(i+1)*width+j].max(table[i*width+j+1])};}}
        let(mut i,mut j)=(0,0);
        while i<x.len()||j<y.len(){
            if i<x.len()&&j<y.len()&&x[i]==y[j]{push(&mut left,x[i],false);push(&mut right,y[j],false);i+=1;j+=1;}
            else if i<x.len()&&(j==y.len()||table[(i+1)*width+j]>=table[i*width+j+1]){push(&mut left,x[i],true);i+=1;}
            else {push(&mut right,y[j],true);j+=1;}
        }
    }
    for ch in &a[a.len()-suffix..]{push(&mut left,*ch,false);push(&mut right,*ch,false);}
    (left,right,coarse)
}
#[cfg(test)]
pub fn layout(runs:&Runs,deletion:bool)->egui::text::LayoutJob{
    let mut job=egui::text::LayoutJob::default();
    for (text,changed) in runs {
        let mut format=egui::TextFormat{font_id:egui::FontId::proportional(16.0),color:Color32::from_rgb(45,45,45),..Default::default()};
        if *changed {format.background=if deletion{Color32::from_rgb(255,195,195)}else{Color32::from_rgb(175,240,185)};if deletion{format.strikethrough=egui::Stroke::new(1.0_f32,Color32::DARK_RED);}}
        job.append(text,0.0,format);
    }job
}
#[cfg(test)]mod tests{
    use super::*;
    fn changed(r:&Runs)->String{r.iter().filter(|r|r.1).map(|r|r.0.as_str()).collect()}
    #[test]fn highlights_only_actual_edits(){
        let(a,b,c)=diff("最新版：tool.exebb\n相同正文。結尾甲🙂","最新版：tool.exe\n相同正文。結尾乙🙂");
        assert!(!c);assert_eq!(changed(&a),"bb甲");assert_eq!(changed(&b),"乙");
        assert_eq!(a.iter().map(|r|r.0.as_str()).collect::<String>(),"最新版：tool.exebb\n相同正文。結尾甲🙂");
        assert!(layout(&a,true).sections.iter().any(|s|s.format.strikethrough.width>0.0));
    }
    #[test]fn same_insert_delete_and_bounded_large_input(){
        assert_eq!(changed(&diff("相同🙂","相同🙂").0),"");
        assert_eq!(changed(&diff("","新增🙂").1),"新增🙂");
        assert_eq!(changed(&diff("刪除🙂","").0),"刪除🙂");
        assert!(diff(&"甲".repeat(5000),&"乙".repeat(5000)).2);
    }
}


use std::{ops::Range, collections::HashMap};
#[derive(Debug,Clone)]
pub struct Change {pub a:Range<usize>,pub b:Range<usize>}
pub struct Comparison {pub a:String,pub b:String,pub changes:Vec<Change>,pub coarse:bool,a_lines:Vec<usize>,b_lines:Vec<usize>}
fn edges(a:&str,b:&str)->(usize,usize){
    let mut prefix=a.bytes().zip(b.bytes()).take_while(|(a,b)|a==b).count();
    while !a.is_char_boundary(prefix)||!b.is_char_boundary(prefix){prefix-=1;}
    let mut suffix=a.as_bytes()[prefix..].iter().rev().zip(b.as_bytes()[prefix..].iter().rev()).take_while(|(a,b)|a==b).count();
    while !a.is_char_boundary(a.len()-suffix)||!b.is_char_boundary(b.len()-suffix){suffix-=1;}
    (prefix,suffix)
}
fn refine(a:&str,b:&str,ao:usize,bo:usize,out:&mut Vec<Change>)->bool{
    let(p,s)=edges(a,b);let a=&a[p..a.len()-s];let b=&b[p..b.len()-s];let(mut ap,mut bp)=(ao+p,bo+p);
    if a.is_empty()&&b.is_empty(){return false;}
    if a.is_empty()||b.is_empty()||a.len().max(b.len())>64_000 {
        out.push(Change{a:ap..ap+a.len(),b:bp..bp+b.len()});return !a.is_empty()&&!b.is_empty();
    }
    let(l,r,coarse)=diff(a,b);
    let left:Vec<_>=l.iter().flat_map(|(t,c)|t.chars().map(move|ch|(ch,*c))).collect();
    let right:Vec<_>=r.iter().flat_map(|(t,c)|t.chars().map(move|ch|(ch,*c))).collect();
    let(mut i,mut j)=(0,0);
    while i<left.len()||j<right.len(){
        if left.get(i).is_some_and(|c|c.1)||right.get(j).is_some_and(|c|c.1){
            let(astart,bstart)=(ap,bp);
            while i<left.len()&&left[i].1{ap+=left[i].0.len_utf8();i+=1;}
            while j<right.len()&&right[j].1{bp+=right[j].0.len_utf8();j+=1;}
            out.push(Change{a:astart..ap,b:bstart..bp});
        }else{ap+=left[i].0.len_utf8();bp+=right[j].0.len_utf8();i+=1;j+=1;}
    }coarse
}
impl Comparison {
    pub fn new(a:String,b:String)->Self{
        let mut changes=Vec::new();let mut coarse=false;
        let(p,s)=edges(&a,&b);let amid=&a[p..a.len()-s];let bmid=&b[p..b.len()-s];
        // Unchanged lines (including repeated lines by occurrence) form monotonic anchors, independent of storage chunk boundaries.
        let al:Vec<&str>=amid.split_inclusive('\n').collect();let bl:Vec<&str>=bmid.split_inclusive('\n').collect();
        let mut positions:HashMap<&str,(Vec<usize>,usize)>=HashMap::new();
        for(i,l)in bl.iter().enumerate(){positions.entry(l).or_default().0.push(i);}
        let mut counts:HashMap<&str,usize>=HashMap::new();for l in &al{*counts.entry(l).or_default()+=1;}
        let mut candidates:Vec<(usize,usize)>=al.iter().enumerate().filter_map(|(i,l)|{
            let entry=positions.get(l)?;(entry.0.len()==1&&counts[l]==1).then(||(i,entry.0[0]))
        }).collect();
        if candidates.is_empty(){
            candidates=al.iter().enumerate().filter_map(|(i,l)|{let entry=positions.get_mut(l)?;let bi=entry.0.get(entry.1).copied()?;entry.1+=1;Some((i,bi))}).collect();
        }
        let mut tails:Vec<usize>=Vec::new();let mut prev=vec![None;candidates.len()];
        for(i,(_,bline))in candidates.iter().enumerate(){let n=tails.partition_point(|t|candidates[*t].1<*bline);if n>0{prev[i]=Some(tails[n-1]);}if n==tails.len(){tails.push(i);}else{tails[n]=i;}}
        let mut anchors=Vec::new();let mut cursor=tails.last().copied();while let Some(i)=cursor{anchors.push(candidates[i]);cursor=prev[i];}anchors.reverse();
        let mut ao=vec![p];for l in &al{ao.push(ao.last().unwrap()+l.len());}let mut bo=vec![p];for l in &bl{bo.push(bo.last().unwrap()+l.len());}
        let(mut ac,mut bc)=(p,p);
        for(ai,bi)in anchors{coarse|=refine(&a[ac..ao[ai]],&b[bc..bo[bi]],ac,bc,&mut changes);ac=ao[ai+1];bc=bo[bi+1];}
        coarse|=refine(&a[ac..a.len()-s],&b[bc..b.len()-s],ac,bc,&mut changes);
        let lines=|s:&str|std::iter::once(0).chain(s.bytes().enumerate().filter_map(|(i,b)|(b==b'\n').then_some(i+1))).collect::<Vec<_>>();
        let a_lines=lines(&a);let b_lines=lines(&b);Self{a,b,changes,coarse,a_lines,b_lines}
    }
    pub fn location(&self,index:usize,left:bool)->(usize,usize){
        let c=&self.changes[index];self.location_at(if left{c.a.start}else{c.b.start},left)
    }
    pub fn location_at(&self,pos:usize,left:bool)->(usize,usize){
        let text=if left{&self.a}else{&self.b};
        let lines=if left{&self.a_lines}else{&self.b_lines};let line=lines.partition_point(|p|*p<=pos).saturating_sub(1);(line+1,text[lines[line]..pos].chars().count()+1)
    }
    #[cfg(test)]
    pub fn view(&self,index:usize,left:bool,font:f32,fg:Color32,flash:bool)->(egui::text::LayoutJob,usize){
        self.view_focus(index,left,font,fg,flash,None)
    }
    pub fn map_range(&self,range:Range<usize>,from_left:bool)->Range<usize>{
        let map=|pos:usize|{let mut delta=0isize;for c in &self.changes{let(a,b)=if from_left{(&c.a,&c.b)}else{(&c.b,&c.a)};if pos<a.start{break;}if pos<a.end{return b.start;}delta=b.end as isize-a.end as isize;}(pos as isize+delta).max(0) as usize};
        let text=if from_left{&self.b}else{&self.a};let mut start=map(range.start).min(text.len());while !text.is_char_boundary(start){start-=1;}let mut end=map(range.end).min(text.len()).max(start);while !text.is_char_boundary(end){end-=1;}start..end
    }
    pub fn view_focus(&self,index:usize,left:bool,font:f32,fg:Color32,flash:bool,focus:Option<Range<usize>>)->(egui::text::LayoutJob,usize){
        let text=if left{&self.a}else{&self.b};
        let searching=focus.is_some();
        let selected=focus.or_else(||self.changes.get(index).map(|c|if left{c.a.clone()}else{c.b.clone()})).unwrap_or(0..0);
        let start=text[..selected.start].char_indices().rev().nth(700).map_or(0,|(i,_)|i);
        let end=text[selected.start..].char_indices().nth(3500).map_or(text.len(),|(i,_)|selected.start+i);
        let mut job=egui::text::LayoutJob::default();let mut target=0;let mut display_chars=0;
        let mut change_idx=self.changes.partition_point(|c|{let r=if left{&c.a}else{&c.b};r.end<start});

        for(pos,ch)in text[start..end].char_indices().map(|(i,c)|(start+i,c)).chain(std::iter::once((end,'\0'))){
            if pos==selected.start{
                target=display_chars;
                if selected.is_empty()&&!self.changes.is_empty(){let marker="⟦此處無文字⟧";job.append(marker,0.0,format(font,fg,true,left,true,flash));display_chars+=marker.chars().count();}
            }
            if pos==end{break;}
            while change_idx<self.changes.len(){let r=if left{&self.changes[change_idx].a}else{&self.changes[change_idx].b};if r.end<=pos{change_idx+=1;}else{break;}}
            let changed=self.changes.get(change_idx).is_some_and(|c|{let r=if left{&c.a}else{&c.b};r.contains(&pos)});
            let active=selected.contains(&pos);
            let display=match ch{'\n'=>{"↵\n".to_string()},'\r'=>"␍".into(),'\t'=>"⇥".into(),_=>{ch.to_string()}};
            display_chars+=display.chars().count();let mut style=format(font,fg,changed||active,left,active,flash);
            if searching&&active{style.background=Color32::from_rgb(255,220,80);style.strikethrough=egui::Stroke::NONE;}
            job.append(&display,0.0,style);
        }
        if end<text.len(){job.append("\n…（後續文字省略；使用上一處／下一處定位）",0.0,format(font,fg,false,left,false,false));}
        (job,target)
    }
}
fn format(font:f32,fg:Color32,changed:bool,left:bool,active:bool,flash:bool)->egui::TextFormat{
    let mut f=egui::TextFormat{font_id:egui::FontId::proportional(font),color:fg,..Default::default()};
    if changed{f.background=if active&&flash{Color32::from_rgb(255,220,80)}else if left{Color32::from_rgb(255,190,180)}else{Color32::from_rgb(165,230,180)};f.color=Color32::from_rgb(25,25,25);if left{f.strikethrough=egui::Stroke::new(1.0_f32,Color32::DARK_RED);}if active{f.underline=egui::Stroke::new(2.0_f32,Color32::from_rgb(130,85,0));}}
    f
}
#[cfg(test)]mod navigation_tests{
    use super::*;
    #[test]fn separate_changes_and_newlines_are_locatable(){
        let c=Comparison::new("第一行甲\n共同第二行\n第三行乙\n".into(),"第一行丙\n共同第二行\n\n第三行丁\n".into());
        assert!(c.changes.len()>=3);assert_eq!(c.location(0,true),(1,4));
        let n=c.changes.iter().position(|h|c.b[h.b.clone()].contains('\n')&&h.a.is_empty()).unwrap();
        let(view,_) =c.view(n,false,24.0,Color32::WHITE,true);assert!(!view.text.contains("空白行"));assert!(view.text.contains('↵'));
        assert_eq!(view.sections[0].format.font_id.size,24.0);
    }
    #[test]fn long_unchanged_text_does_not_become_a_change_after_insertion(){
        let common=(0..15000).map(|n|format!("共同第{n}行，原始內容。\n")).collect::<String>();
        let a=format!("原文\n{common}尾部甲");let b=format!("新增\n原文\n{common}尾部乙");let c=Comparison::new(a,b);
        assert_eq!(c.changes.len(),2);assert_eq!(&c.b[c.changes[0].b.clone()],"新增\n");assert!(!c.coarse);
    }
}

#[cfg(test)]mod repeated_tests{
    use super::*;
    #[test]fn repeated_lines_still_yield_separate_edits(){
        let common="共同段落，完全相同。\n".repeat(10000);
        let c=Comparison::new(format!("開頭甲\n{common}結尾甲"),format!("開頭乙\n{common}結尾乙"));
        assert_eq!(c.changes.len(),2);assert!(!c.coarse);
    }
}

#[cfg(test)]mod repeated_separator_regression{
    use super::*;
    #[test]fn small_edits_with_repeated_separators_do_not_expand(){
        let common=(0..200).map(|n|format!("\n==========\n\n第{n}章 獨立章名\n\n正文第{n}段\n")).collect::<String>();
        let a=format!("作者1\n{common}結尾?\n");let b=format!("作者\n{common}結尾\n");
        let c=Comparison::new(a,b);assert_eq!(c.changes.len(),2);
    }
}
