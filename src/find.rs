use eframe::egui;
use std::{sync::{Arc,mpsc},ops::Range};
#[derive(Clone,Debug)]pub struct Hit{pub side:usize,pub range:Range<usize>}
#[derive(Default)]pub struct Find{
    pub open:bool,query:String,source:String,hits:Vec<Hit>,selected:usize,
    receiver:Option<mpsc::Receiver<Vec<Hit>>>,focus:bool,pending_jump:bool,
}
impl Find{
    pub fn source(&mut self,key:String)->bool{if self.source!=key{self.source=key;self.hits.clear();self.receiver=None;self.pending_jump=false;true}else{false}}
    pub fn shortcut(&mut self,ctx:&egui::Context){if ctx.input_mut(|i|i.consume_key(egui::Modifiers::CTRL,egui::Key::F)){self.open=true;self.focus=true;}}
    pub fn ui(&mut self,ui:&mut egui::Ui,sources:impl FnOnce()->Vec<Vec<Arc<String>>>)->Option<Hit>{
        if let Some(rx)=&self.receiver{if let Ok(hits)=rx.try_recv(){self.receiver=None;self.hits=hits;self.selected=0;self.pending_jump=true;}}
        if !self.open{return None;}
        let mut search=false;
        ui.horizontal_wrapped(|ui|{
            ui.label("全文尋找");let response=ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("輸入文字，Enter 尋找").desired_width(240.0));
            if self.focus{response.request_focus();self.focus=false;}
            if response.changed(){self.hits.clear();self.receiver=None;}
            let enter=ui.input(|i|i.key_pressed(egui::Key::Enter))&&(response.has_focus()||response.lost_focus());
            if ui.button("尋找").clicked()||enter{search=true;}
            if ui.add_enabled(!self.hits.is_empty(),egui::Button::new("上一筆")).clicked(){self.selected=(self.selected+self.hits.len()-1)%self.hits.len();self.pending_jump=true;}
            if ui.add_enabled(!self.hits.is_empty(),egui::Button::new("下一筆")).clicked(){self.selected=(self.selected+1)%self.hits.len();self.pending_jump=true;}
            if self.receiver.is_some(){ui.spinner();}else{ui.label(format!("{} / {}{}",if self.hits.is_empty(){0}else{self.selected+1},self.hits.len(),if self.hits.len()==10000{"（最多顯示一萬筆）"}else{""}));}
            if ui.button("關閉尋找").clicked(){self.open=false;}
        });
        if search&&!self.query.is_empty(){let sources=sources();let query=self.query.clone();let(tx,rx)=mpsc::channel();self.receiver=Some(rx);let ctx=ui.ctx().clone();std::thread::spawn(move||{let _=tx.send(search_sources(sources,&query));ctx.request_repaint();});}
        if self.pending_jump{self.pending_jump=false;self.hits.get(self.selected).cloned()}else{None}
    }
}
pub fn search_sources(sources:Vec<Vec<Arc<String>>>,query:&str)->Vec<Hit>{
    let mut hits=Vec::new();if query.is_empty(){return hits;}
    for(side,chunks)in sources.into_iter().enumerate(){
        let text=chunks.iter().map(|s|s.as_str()).collect::<String>();
        for(start,matched)in text.match_indices(query){hits.push(Hit{side,range:start..start+matched.len()});if hits.len()==10000{return hits;}}
    }hits
}
#[cfg(test)]mod tests{use super::*;#[test]fn finds_across_chunks_and_both_versions(){let h=search_sources(vec![vec![Arc::new("甲第".into()),Arc::new("三章乙".into())],vec![Arc::new("第三章".into())]],"第三章");assert_eq!(h.len(),2);assert_eq!(h[0].range,3..12);assert_eq!(h[1].side,1);}}
