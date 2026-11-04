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
