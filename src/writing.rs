use crate::writing_store::{self as store, Document};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::{mpsc, Arc}, time::{Duration, Instant}};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences { pub enabled: bool, pub last: Option<PathBuf>, pub page: usize, pub autosave_secs: u64, pub limit_mib: u64, pub compare_left: f32, pub compare_right: f32 }
impl Default for Preferences { fn default() -> Self { Self { enabled: false, last: None, page: 0, autosave_secs: 60, limit_mib: 1024, compare_left: 0.0, compare_right: 0.0 } } }
enum Completion { Document(Document), Compare(crate::version_diff::Comparison) }
#[derive(Clone)]
enum After { Nothing, Close, Read, Open(PathBuf, bool) }
pub struct Writer {
    pub preferences: Preferences, pub doc: Option<Document>, pub message: String,
    pub read_ready: bool, pub allow_close: bool, pub close_failed: bool, close_pending: bool,
    root: PathBuf, receiver: Option<mpsc::Receiver<store::Result<Completion>>>, after: After,
    last_auto: Instant, pub history_open: bool, comparison_tab: bool, left_id: u64, right_id: u64,
    comparison: Option<crate::version_diff::Comparison>, comparison_ids:(u64,u64), selected_change:usize,
    highlight_since: Instant, scroll_change: bool,
    confirm_delete: Option<u64>, jump: usize,
}
impl Writer {
    pub fn new(root: PathBuf, preferences: Preferences) -> Self {
        Self { preferences, root, doc: None, message: String::new(), read_ready: false, allow_close: false, close_failed:false, close_pending: false, receiver: None, after: After::Nothing, last_auto: Instant::now(), history_open: false, comparison_tab:false, left_id: 0, right_id: 0, comparison: None, comparison_ids:(0,0), selected_change:0, highlight_since:Instant::now(), scroll_change:true, confirm_delete: None, jump: 1 }
    }
    pub fn busy(&self) -> bool { self.receiver.is_some() }
    fn task(&mut self, ctx: &egui::Context, after: After, work: impl FnOnce() -> store::Result<Completion> + Send + 'static) {
        let (tx,rx)=mpsc::channel(); self.receiver=Some(rx); self.after=after;
        let ctx=ctx.clone(); std::thread::spawn(move || { let result=work(); let _=tx.send(result); ctx.request_repaint(); });
    }
    pub fn open(&mut self, path: PathBuf, create: bool, ctx: &egui::Context) {
        if self.busy() { return; }
        if self.doc.as_ref().is_some_and(|d| crate::key(&d.path)==crate::key(&path)) { return; }
        if self.doc.as_ref().is_some_and(|d| d.dirty) {
            self.save(None,After::Open(path,create),ctx); return;
        }
        self.begin_open(path,create,ctx);
    }
    fn begin_open(&mut self,path:PathBuf,create:bool,ctx:&egui::Context) {
        let root=self.root.clone(); self.message="背景載入寫作文件與恢復草稿…".into();
        self.task(ctx,After::Nothing,move||store::open(&root,path,create).map(Completion::Document));
    }
    pub fn request_close(&mut self,ctx:&egui::Context){
        self.close_failed=false;self.allow_close=false;
        if !self.preferences.enabled {self.allow_close=true;return;}
        if self.busy(){self.close_pending=true;}else{self.save(Some("關閉儲存"),After::Close,ctx);}
    }
    pub fn leave(&mut self,ctx:&egui::Context) {
        if self.busy(){return;}
        self.history_open=false;
        if self.doc.as_ref().is_some_and(|d|d.dirty){ self.save(None,After::Read,ctx); }
        else { self.read_ready=true; }
    }
    fn save(&mut self, reason:Option<&str>,after:After,ctx:&egui::Context) {
        if self.busy(){return;}
        let Some(doc)=self.doc.clone() else { if matches!(after,After::Close){self.allow_close=true;} return; };
        self.message=if reason.is_some(){"正在儲存文件及建立版本…"}else{"正在更新恢復草稿（不建立版本）…"}.into();
        let reason=reason.map(str::to_owned);
        self.task(ctx,after,move||store::save(doc,reason.as_deref()).map(Completion::Document));
    }
    pub fn manual_save(&mut self,ctx:&egui::Context) { self.save(Some("手動儲存"),After::Nothing,ctx); }
    pub fn save_as_to(&mut self,path:PathBuf,ctx:&egui::Context){
        let Some(doc)=self.doc.clone() else{return;};
        if crate::key(&path)==crate::key(&doc.path){self.manual_save(ctx);return;}
        let root=self.root.clone();
        self.task(ctx,After::Nothing,move||{
            if doc.dirty{store::save(doc.clone(),None)?;}
            let mut target=store::open(&root,path,true)?;
            target.chunks=doc.chunks;target.encoding=doc.encoding;target.page=doc.page;target.revision+=1;
            store::save(target,Some("另存新檔")).map(Completion::Document)
        });
    }
    pub fn tick(&mut self,ctx:&egui::Context,active:bool) {
        if let Some(rx)=&self.receiver {
            match rx.try_recv() {
                Ok(result)=>{
                    self.receiver=None; let after=std::mem::replace(&mut self.after,After::Nothing);
                    let after=if self.close_pending{After::Nothing}else{after};
                    match result {
                        Ok(Completion::Document(mut doc))=>{
                            let was_same=self.doc.as_ref().is_some_and(|old|old.path==doc.path);
                            if !was_same {self.comparison_tab=false;self.comparison=None;self.left_id=0;self.right_id=0;self.confirm_delete=None;}
                            if !was_same && self.preferences.last.as_ref().is_some_and(|p|crate::key(p)==crate::key(&doc.path)) {doc.page=self.preferences.page.min(doc.chunks.len()-1);}
                            self.preferences.last=Some(doc.path.clone());self.preferences.page=doc.page;self.jump=doc.page+1;
                            self.message=if doc.dirty{"恢復草稿已保存／載入；原 TXT 尚未更新"}else{"文件已就緒；手動儲存或關閉將建立版本"}.into();
                            if doc.version_skipped {self.message="已儲存；相同內容最多保留兩個連續版本，不再新增重複節點".into();}
                            self.doc=Some(doc);self.last_auto=Instant::now();
                            match after {After::Close=>{self.allow_close=true;},After::Read=>self.read_ready=true,After::Open(path,create)=>self.begin_open(path,create,ctx),After::Nothing=>{}}
                        }
                        Ok(Completion::Compare(comparison))=>{
                            self.comparison_tab=true;self.comparison=Some(comparison);self.selected_change=0;self.scroll_change=true;self.highlight_since=Instant::now();self.message="全文差異已就緒".into();
                        }
                        Err(error)=>{self.close_failed=matches!(after,After::Close)||self.close_pending;self.close_pending=false;self.message=format!("操作失敗，文件保持開啟：{error}");self.last_auto=Instant::now();}
                    }
                }
                Err(mpsc::TryRecvError::Empty)=>{ctx.request_repaint_after(Duration::from_millis(100));}
                Err(mpsc::TryRecvError::Disconnected)=>{self.receiver=None;self.close_failed=true;self.close_pending=false;self.after=After::Nothing;self.message="背景操作中斷，請重試；文件保持開啟".into();}
            }
        }
        if let Some(doc)=&self.doc {self.preferences.page=doc.page;}
        if self.preferences.enabled {
            if self.close_pending&&!self.busy(){self.close_pending=false;self.save(Some("關閉儲存"),After::Close,ctx);}
            if active && !self.history_open && !self.busy() && ctx.input_mut(|i|i.consume_key(egui::Modifiers::CTRL,egui::Key::S)){self.manual_save(ctx);}
            if !self.allow_close && autosave_due(self.preferences.enabled,self.doc.as_ref().map(|d|(d.revision,d.drafted_revision)),self.last_auto.elapsed(),self.preferences.autosave_secs) && !self.busy(){self.save(None,After::Nothing,ctx);}
        }
    }
    pub fn toolbar(&mut self,ui:&mut egui::Ui,ctx:&egui::Context) {
        let busy=self.busy();ui.add_enabled_ui(!busy,|ui|{
            if ui.add_enabled(self.doc.is_some(),egui::Button::new("儲存 Ctrl+S")).clicked(){self.manual_save(ctx);}
            if ui.button("版本").clicked(){self.history_open=!self.history_open;if self.history_open{self.comparison_tab=false;}}
        });if busy{ui.spinner();}
    }
    pub fn settings(&mut self,ui:&mut egui::Ui){
        ui.horizontal_wrapped(|ui|{ui.label("自動草稿間隔");ui.add(egui::DragValue::new(&mut self.preferences.autosave_secs).range(5..=3600).suffix(" 秒"));ui.label("歷史提醒上限");ui.add(egui::DragValue::new(&mut self.preferences.limit_mib).range(1..=102400).suffix(" MiB"));});
        ui.small("自動儲存只更新最新恢復草稿；手動儲存／關閉才建立版本。設定為所有作品共用。");
    }
    pub fn sidebar(&mut self,ui:&mut egui::Ui){
        ui.heading("寫作區塊");ui.small("大型文件分塊編輯；儲存會依序合併全部內容。");
        let busy=self.busy();
        if let Some(doc)=&mut self.doc{
            egui::ScrollArea::vertical().id_salt("writing-chunks").show_rows(ui,26.0,doc.chunks.len(),|ui,range|{
                for n in range{let title=doc.chunks[n].lines().find(|l|!l.trim().is_empty()).unwrap_or("（空白）").chars().take(28).collect::<String>();
                    if ui.add_enabled(!busy,egui::Button::new(format!("{}. {title}",n+1)).selected(doc.page==n).truncate()).clicked(){doc.page=n;}
                }
            });
        }
    }
    pub fn editor(&mut self,ui:&mut egui::Ui,font:f32,foreground:egui::Color32,background:egui::Color32){
        *ui.visuals_mut()=document_visuals(background,foreground);
        let busy=self.busy();
        let Some(doc)=&mut self.doc else{ui.heading("寫作模式");ui.label("開啟 TXT 或按「新建」開始寫作。");return;};
        ui.horizontal_wrapped(|ui|{
            ui.strong(doc.path.file_name().unwrap_or_default().to_string_lossy());
            ui.label(if doc.dirty{"● 有未寫回原檔的修改"}else{"已儲存"});
            if ui.add_enabled(!busy&&doc.page>0,egui::Button::new("上一塊")).clicked(){doc.page-=1;}
            if ui.add_enabled(!busy&&doc.page+1<doc.chunks.len(),egui::Button::new("下一塊")).clicked(){doc.page+=1;}
            ui.label(format!("{} / {}",doc.page+1,doc.chunks.len()));
            ui.add(egui::DragValue::new(&mut self.jump).range(1..=doc.chunks.len()));
            if ui.add_enabled(!busy,egui::Button::new("跳轉")).clicked(){doc.page=self.jump.saturating_sub(1).min(doc.chunks.len()-1);}
        });
        ui.small("區塊界線不是正文換行；保存會原樣接合。貼上較長文字後自動分塊。");
        egui::ScrollArea::vertical().id_salt(("writer",doc.path.clone(),doc.page)).show(ui,|ui|{
            let text=Arc::make_mut(&mut doc.chunks[doc.page]);
            let response=ui.add_enabled(!busy,egui::TextEdit::multiline(text).id_salt(("writing-text",doc.path.clone(),doc.page)).font(egui::FontId::proportional(font)).text_color(foreground).background_color(background).desired_width(f32::INFINITY).desired_rows(20));
            if response.changed(){doc.revision+=1;doc.dirty=true;
                if text.chars().count()>store::CHUNK_CHARS*2{let parts=store::split(text);doc.chunks.splice(doc.page..=doc.page,parts);}
            }
        });
    }
    pub fn status(&self,ui:&mut egui::Ui){
        ui.label(&self.message);
        if let Some(doc)=&self.doc{
            let mib=doc.history_bytes as f64/1048576.0;
            let text=format!("{} 個版本 · 歷史 {:.2} MiB / {} MiB",doc.history.versions.len(),mib,self.preferences.limit_mib);
            if mib>self.preferences.limit_mib as f64{ui.colored_label(egui::Color32::from_rgb(200,80,20),format!("超過提醒上限，請手動清理 · {text}"));}else{ui.label(text);}
        }
    }
    pub fn history_ui(&mut self,ctx:&egui::Context,settings:&crate::Settings){
        let bg=egui::Color32::from_rgba_unmultiplied(settings.background[0],settings.background[1],settings.background[2],settings.background[3]);
        let fg=egui::Color32::from_rgba_unmultiplied(settings.foreground[0],settings.foreground[1],settings.foreground[2],settings.foreground[3]);
        let mut action=None;let mut compare=false;let busy=self.busy();
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(bg).inner_margin(12.0)).show(ctx,|ui|{
            *ui.visuals_mut()=document_visuals(bg,fg);
            for font in ui.style_mut().text_styles.values_mut(){font.size=settings.font;}
            ui.horizontal_wrapped(|ui|{ui.heading("版本管理與對比");if ui.button("退出版本管理").clicked(){self.history_open=false;}if busy{ui.spinner();}});
            if ctx.input_mut(|i|i.consume_key(egui::Modifiers::NONE,egui::Key::Escape)){self.history_open=false;}
            let Some(doc)=&self.doc else{ui.label("先開啟寫作文件");return;};
            ui.label(doc.path.file_name().unwrap_or_default().to_string_lossy());
            if self.message.starts_with("操作失敗") {ui.colored_label(egui::Color32::LIGHT_RED,&self.message);}
            ui.horizontal_wrapped(|ui|{
                if ui.selectable_label(!self.comparison_tab,"版本列表").clicked(){self.comparison_tab=false;}
                let title=comparison_title(&doc.path,self.comparison_ids);
                if ui.add_enabled(self.comparison.is_some()||self.comparison_tab,egui::Button::new(title).selected(self.comparison_tab)).clicked(){self.comparison_tab=true;self.scroll_change=true;}
            });
            ui.separator();
            if !self.comparison_tab {
                if ui.add_enabled(!busy&&self.left_id>0&&self.right_id>0,egui::Button::new("比較 A / B")).clicked(){compare=true;}
            if let Some(id)=self.confirm_delete{ui.horizontal_wrapped(|ui|{ui.label(format!("確定刪除 #{id}？"));if ui.add_enabled(!busy,egui::Button::new("確定刪除")).clicked(){action=Some((true,id));self.confirm_delete=None;}if ui.button("取消").clicked(){self.confirm_delete=None;}});}
            let row_height=settings.font+10.0;
            egui::ScrollArea::vertical().id_salt("versions-list").max_height(ui.available_height()).show_rows(ui,row_height,doc.history.versions.len(),|ui,range|{
                for i in range{let v=&doc.history.versions[doc.history.versions.len()-1-i];ui.horizontal(|ui|{
                    ui.selectable_value(&mut self.left_id,v.id,format!("A #{}",v.id));ui.selectable_value(&mut self.right_id,v.id,format!("B #{}",v.id));
                    ui.label(format!("{} · {}",v.time,v.reason));
                    if ui.add_enabled(!busy,egui::Button::new("預覽")).clicked(){self.left_id=v.id;self.right_id=v.id;compare=true;}
                    if ui.add_enabled(!busy,egui::Button::new("還原")).clicked(){action=Some((false,v.id));}
                    if ui.add_enabled(!busy,egui::Button::new("刪除")).clicked(){self.confirm_delete=Some(v.id);}
                });}
            });
                return;
            }
            ui.horizontal_wrapped(|ui|{
                ui.label("對比專用 · 左留白");
                let left=ui.add(egui::DragValue::new(&mut self.preferences.compare_left).range(0.0..=500.0).speed(2.0).suffix(" px")).changed();
                ui.label("右留白");
                let right=ui.add(egui::DragValue::new(&mut self.preferences.compare_right).range(0.0..=500.0).speed(2.0).suffix(" px")).changed();
                if left||right{self.scroll_change=true;}
            });
            ui.horizontal_wrapped(|ui|{
                if let Some(c)=&self.comparison{
                    let count=c.changes.len();
                    if ui.add_enabled(count>0,egui::Button::new("上一處差異")).clicked(){self.selected_change=(self.selected_change+count-1)%count;self.scroll_change=true;self.highlight_since=Instant::now();}
                    if ui.add_enabled(count>0,egui::Button::new("下一處差異")).clicked(){self.selected_change=(self.selected_change+1)%count;self.scroll_change=true;self.highlight_since=Instant::now();}
                    ui.label(if count==0{"內容相同".into()}else{format!("差異 {} / {}",self.selected_change+1,count)});
                }
            });
            ui.label("↵ 換行 · 〔空白行〕空行 · 紅底刪除／綠底新增 · 底線為目前差異");
            if let Some(c)=&self.comparison{
                let elapsed=self.highlight_since.elapsed().as_secs_f32();let flash=elapsed<0.9&&(elapsed*6.0) as usize%2==0;
                if elapsed<0.9{ctx.request_repaint_after(Duration::from_millis(70));}
                if c.coarse{ui.label("大量替換範圍以連續增刪表示。");}
                let height=ui.available_height().max(80.0);
                ui.columns(2,|cols|{for(side,col)in cols.iter_mut().enumerate(){
                    let left=side==0;let id=if left{self.comparison_ids.0}else{self.comparison_ids.1};
                    let location=if c.changes.is_empty(){(1,1)}else{c.location(self.selected_change,left)};
                    col.label(format!("{} #{id} · 第 {} 行，第 {} 字",if left{"A"}else{"B"},location.0,location.1));
                    egui::ScrollArea::vertical().id_salt(("full-diff",side)).max_height((height-settings.font-12.0).max(40.0)).show(col,|ui|{
                        let (mut job,target)=c.view(self.selected_change,left,settings.font,fg,flash);
                        for section in &mut job.sections{section.format.line_height=Some(settings.font*settings.line_spacing);}
                        let (lm,rm)=crate::reading_margins(ui.available_width(),self.preferences.compare_left,self.preferences.compare_right);
                        ui.horizontal(|ui|{ui.add_space(lm);ui.vertical(|ui|{
                            let width=(ui.available_width()-rm).max(80.0);ui.set_width(width);job.wrap.max_width=width;
                            let galley=crate::paragraph_layout(ui.fonts(|f|f.layout_job(job)),settings.paragraph_gap);
                            let cursor=galley.from_ccursor(egui::text::CCursor::new(target));let caret=galley.pos_from_cursor(&cursor);
                            let response=ui.add(egui::Label::new(galley));
                            if self.scroll_change{ui.scroll_to_rect(caret.translate(response.rect.min.to_vec2()),Some(egui::Align::Center));}
                        });});
                    });
                }});self.scroll_change=false;
            }else{ui.label(if busy{"正在比對全文…"}else{"尚未載入比較結果，請回版本列表選擇 A 與 B。"});}
        });
        if compare&&!busy{if let Some(doc)=&self.doc{
            let a=doc.history.versions.iter().find(|v|v.id==self.left_id).map(|v|v.snapshot.clone());let b=doc.history.versions.iter().find(|v|v.id==self.right_id).map(|v|v.snapshot.clone());
            if let(Some(a),Some(b))=(a,b){let store=doc.store.clone();self.comparison_ids=(self.left_id,self.right_id);self.comparison=None;self.comparison_tab=true;self.message="正在比對全文…".into();self.task(ctx,After::Nothing,move||{
                let a=store::read_snapshot(&store,&a)?.iter().map(|s|s.as_str()).collect::<String>();
                let b=store::read_snapshot(&store,&b)?.iter().map(|s|s.as_str()).collect::<String>();
                Ok(Completion::Compare(crate::version_diff::Comparison::new(a,b)))
            });}
        }}
        if let Some((delete,id))=action{if let Some(doc)=self.doc.clone(){self.task(ctx,After::Nothing,move||if delete{store::delete(doc,id).map(Completion::Document)}else{store::restore(doc,id).map(Completion::Document)});}}
    }

}
fn comparison_title(path:&std::path::Path,ids:(u64,u64))->String{
    if ids.0==0||ids.1==0{return "對比（尚未選擇）".into();}
    format!("{}#{} #{}對比",path.file_name().unwrap_or_default().to_string_lossy(),ids.0,ids.1)
}
fn document_visuals(bg:egui::Color32,fg:egui::Color32)->egui::Visuals{
    let mut visuals=if u16::from(bg.r())+u16::from(bg.g())+u16::from(bg.b())<380{egui::Visuals::dark()}else{egui::Visuals::light()};
    visuals.override_text_color=Some(fg);visuals.panel_fill=bg;visuals.extreme_bg_color=bg;visuals.widgets.noninteractive.fg_stroke.color=fg;visuals
}
pub fn autosave_due(enabled:bool,revisions:Option<(u64,u64)>,elapsed:Duration,seconds:u64)->bool{
    enabled&&revisions.is_some_and(|(current,saved)|current!=saved)&&elapsed>=Duration::from_secs(seconds.clamp(5,3600))
}
#[cfg(test)]mod tests{use super::*;#[test]fn only_modified_writing_autosaves(){assert!(!autosave_due(false,Some((2,1)),Duration::from_secs(100),60));assert!(!autosave_due(true,Some((1,1)),Duration::from_secs(100),60));assert!(!autosave_due(true,Some((2,1)),Duration::from_secs(59),60));assert!(autosave_due(true,Some((2,1)),Duration::from_secs(60),60));assert!(!Preferences::default().enabled);assert_eq!(Preferences::default().autosave_secs,60);}}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn close_waits_for_background_draft_then_creates_manual_version() {
        let root=PathBuf::from("verification").join(format!("writer-close-{}",std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path=root.join("close.txt");std::fs::write(&path,"original").unwrap();
        let ctx=egui::Context::default();
        let mut writer=Writer::new(root.join(format!("history-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())),Preferences{enabled:true,..Default::default()});
        let mut doc=store::open(&writer.root,path.clone(),false).unwrap();
        doc.chunks=store::split("modified");doc.revision=1;doc.dirty=true;writer.doc=Some(doc);
        writer.save(None,After::Nothing,&ctx);writer.close_pending=true;
        let start=Instant::now();
        while !writer.allow_close && start.elapsed()<Duration::from_secs(10) {
            let _=ctx.run(Default::default(),|ctx|writer.tick(ctx,true));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(writer.allow_close);assert_eq!(std::fs::read_to_string(path).unwrap(),"modified");
        let doc=writer.doc.as_ref().unwrap();assert_eq!(doc.history.versions.len(),1);assert!(doc.history.draft.is_none());
        assert_eq!(doc.history.versions[0].reason,"關閉儲存");
    }
}

#[cfg(test)]mod history_tab_tests{
    use super::*;
    #[test]fn independent_margins_default_and_persist(){
        let old:Preferences=serde_json::from_str("{}").unwrap();assert_eq!((old.compare_left,old.compare_right),(0.0,0.0));
        let prefs=Preferences{compare_left:24.0,compare_right:56.0,..Default::default()};
        let restored:Preferences=serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!((restored.compare_left,restored.compare_right),(24.0,56.0));
        assert_eq!(comparison_title(std::path::Path::new("使用說明.txt"),(1,8)),"使用說明.txt#1 #8對比");
    }
}
