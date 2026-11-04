use crate::writing_store::{self as store, Chunks, Document};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::{mpsc, Arc}, time::{Duration, Instant}};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences { pub enabled: bool, pub last: Option<PathBuf>, pub page: usize, pub autosave_secs: u64, pub limit_mib: u64 }
impl Default for Preferences { fn default() -> Self { Self { enabled: false, last: None, page: 0, autosave_secs: 60, limit_mib: 1024 } } }
enum Completion { Document(Document), Compare(Chunks, Chunks) }
#[derive(Clone)]
enum After { Nothing, Close, Read, Open(PathBuf, bool) }
pub struct Writer {
    pub preferences: Preferences, pub doc: Option<Document>, pub message: String,
    pub read_ready: bool, pub allow_close: bool, close_pending: bool,
    root: PathBuf, receiver: Option<mpsc::Receiver<store::Result<Completion>>>, after: After,
    last_auto: Instant, history_open: bool, left_id: u64, right_id: u64,
    comparison: Option<(Chunks, Chunks)>, comparison_ids: (u64,u64), differences: Vec<usize>, diff_page: usize,
    diff_cache: Option<(usize,egui::text::LayoutJob,egui::text::LayoutJob,bool)>,
    confirm_delete: Option<u64>, jump: usize,
}
impl Writer {
    pub fn new(root: PathBuf, preferences: Preferences) -> Self {
        Self { preferences, root, doc: None, message: String::new(), read_ready: false, allow_close: false, close_pending: false, receiver: None, after: After::Nothing, last_auto: Instant::now(), history_open: false, left_id: 0, right_id: 0, comparison: None, comparison_ids: (0,0), differences: vec![], diff_page: 0, diff_cache: None, confirm_delete: None, jump: 1 }
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
    pub fn choose_new(&mut self,ctx:&egui::Context) {
        if let Some(path)=rfd::FileDialog::new().add_filter("文字檔", &["txt"]).set_file_name("新作品.txt").save_file() { self.open(path,true,ctx); }
    }
    pub fn leave(&mut self,ctx:&egui::Context) {
        if self.busy(){return;}
        self.history_open=false;
        if self.doc.as_ref().is_some_and(|d|d.dirty){ self.save(None,After::Read,ctx); }
        else { self.read_ready=true; }
    }
    fn save(&mut self, reason:Option<&str>,after:After,ctx:&egui::Context) {
        if self.busy(){return;}
        let Some(doc)=self.doc.clone() else { if matches!(after,After::Close){self.allow_close=true;ctx.send_viewport_cmd(egui::ViewportCommand::Close);} return; };
        self.message=if reason.is_some(){"正在儲存文件及建立版本…"}else{"正在更新恢復草稿（不建立版本）…"}.into();
        let reason=reason.map(str::to_owned);
        self.task(ctx,after,move||store::save(doc,reason.as_deref()).map(Completion::Document));
    }
    pub fn manual_save(&mut self,ctx:&egui::Context) { self.save(Some("手動儲存"),After::Nothing,ctx); }
    fn save_as(&mut self,ctx:&egui::Context) {
        let Some(doc)=self.doc.clone() else{return;};
        if let Some(path)=rfd::FileDialog::new().add_filter("文字檔", &["txt"]).set_file_name(doc.path.file_name().unwrap_or_default().to_string_lossy()).save_file(){
            if crate::key(&path)==crate::key(&doc.path){self.manual_save(ctx);return;}
            let root=self.root.clone();
            self.task(ctx,After::Nothing,move||{
                // Preserve unsaved work in the original document's recovery draft too.
                if doc.dirty { store::save(doc.clone(),None)?; }
                let mut target=store::open(&root,path,true)?;
                target.chunks=doc.chunks;target.encoding=doc.encoding;target.page=doc.page;target.revision+=1;
                store::save(target,Some("另存新檔")).map(Completion::Document)
            });
        }
    }
    pub fn tick(&mut self,ctx:&egui::Context) {
        if let Some(rx)=&self.receiver {
            match rx.try_recv() {
                Ok(result)=>{
                    self.receiver=None; let after=std::mem::replace(&mut self.after,After::Nothing);
                    let after=if self.close_pending{After::Nothing}else{after};
                    match result {
                        Ok(Completion::Document(mut doc))=>{
                            let was_same=self.doc.as_ref().is_some_and(|old|old.path==doc.path);
                            if !was_same {self.comparison=None;self.left_id=0;self.right_id=0;self.confirm_delete=None;}
                            if !was_same && self.preferences.last.as_ref().is_some_and(|p|crate::key(p)==crate::key(&doc.path)) {doc.page=self.preferences.page.min(doc.chunks.len()-1);}
                            self.preferences.last=Some(doc.path.clone());self.preferences.page=doc.page;self.jump=doc.page+1;
                            self.message=if doc.dirty{"恢復草稿已保存／載入；原 TXT 尚未更新"}else{"文件已就緒；手動儲存或關閉將建立版本"}.into();
                            if doc.version_skipped {self.message="已儲存；相同內容最多保留兩個連續版本，不再新增重複節點".into();}
                            self.doc=Some(doc);self.last_auto=Instant::now();
                            match after {After::Close=>{self.allow_close=true;ctx.send_viewport_cmd(egui::ViewportCommand::Close);},After::Read=>self.read_ready=true,After::Open(path,create)=>self.begin_open(path,create,ctx),After::Nothing=>{}}
                        }
                        Ok(Completion::Compare(left,right))=>{
                            self.differences=(0..left.len().max(right.len())).filter(|i|left.get(*i)!=right.get(*i)).collect();
                            self.diff_cache=None;self.comparison=Some((left,right));self.diff_page=0;self.message="版本已載入，可查看差異區塊".into();
                        }
                        Err(error)=>{self.close_pending=false;self.message=format!("操作失敗，文件保持開啟：{error}");self.last_auto=Instant::now();}
                    }
                }
                Err(mpsc::TryRecvError::Empty)=>{ctx.request_repaint_after(Duration::from_millis(100));}
                Err(mpsc::TryRecvError::Disconnected)=>{self.receiver=None;self.close_pending=false;self.after=After::Nothing;self.message="背景操作中斷，請重試；文件保持開啟".into();}
            }
        }
        if let Some(doc)=&self.doc {self.preferences.page=doc.page;}
        if self.preferences.enabled {
            if ctx.input(|i|i.viewport().close_requested()) && !self.allow_close {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                if !self.busy(){self.save(Some("關閉儲存"),After::Close,ctx);}
                else {self.close_pending=true;self.message="背景操作完成後將儲存版本並關閉…".into();}
            }
            if self.close_pending&&!self.busy(){self.close_pending=false;self.save(Some("關閉儲存"),After::Close,ctx);}
            if !self.busy() && ctx.input_mut(|i|i.consume_key(egui::Modifiers::CTRL,egui::Key::S)){self.manual_save(ctx);}
            if autosave_due(self.preferences.enabled,self.doc.as_ref().map(|d|(d.revision,d.drafted_revision)),self.last_auto.elapsed(),self.preferences.autosave_secs) && !self.busy(){self.save(None,After::Nothing,ctx);}
        }
    }
    pub fn toolbar(&mut self,ui:&mut egui::Ui,ctx:&egui::Context) {
        let busy=self.busy();ui.add_enabled_ui(!busy,|ui|{
            if ui.button("新建").clicked(){self.choose_new(ctx);}
            if ui.add_enabled(self.doc.is_some(),egui::Button::new("儲存 Ctrl+S")).clicked(){self.manual_save(ctx);}
            if ui.add_enabled(self.doc.is_some(),egui::Button::new("另存")).clicked(){self.save_as(ctx);}
            if ui.button("版本").clicked(){self.history_open=!self.history_open;}
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
    pub fn history_ui(&mut self,ctx:&egui::Context){
        if !self.history_open{return;}
        let mut open=true;let mut action=None;let mut compare=false;
        egui::Window::new("版本管理與對比").open(&mut open).default_size([1000.0,700.0]).show(ctx,|ui|{
            let Some(doc)=&self.doc else{ui.label("先開啟寫作文件");return;};
            ui.small(format!("版本與草稿：{}",doc.store.display()));
            ui.label("版本時間為 Unix 秒；還原前會自動建立目前內容的備份版本。刪除只移除歷史，不改原TXT。");
            let busy=self.busy();
            egui::ScrollArea::vertical().max_height(160.0).show_rows(ui,26.0,doc.history.versions.len(),|ui,range|{
                for i in range{let v=&doc.history.versions[doc.history.versions.len()-1-i];ui.horizontal(|ui|{
                    ui.selectable_value(&mut self.left_id,v.id,format!("A #{}",v.id));ui.selectable_value(&mut self.right_id,v.id,format!("B #{}",v.id));
                    ui.label(format!("{} · {} · {} 區塊",v.time,v.reason,v.snapshot.chunks.len()));
                    if ui.add_enabled(!busy,egui::Button::new("預覽")).clicked(){self.left_id=v.id;self.right_id=v.id;compare=true;}
                    if ui.add_enabled(!busy,egui::Button::new("還原")).clicked(){action=Some((false,v.id));}
                    if ui.add_enabled(!busy,egui::Button::new("刪除")).clicked(){self.confirm_delete=Some(v.id);}
                });}
            });
            if let Some(id)=self.confirm_delete{ui.horizontal(|ui|{ui.label(format!("確定刪除版本 #{id}？"));if ui.add_enabled(!busy,egui::Button::new("確定刪除")).clicked(){action=Some((true,id));self.confirm_delete=None;}if ui.button("取消").clicked(){self.confirm_delete=None;}});}
            if ui.add_enabled(!busy&&self.left_id>0&&self.right_id>0,egui::Button::new("比較 A / B")).clicked(){compare=true;}
            if let Some((left,right))=&self.comparison{
                ui.label(format!("A #{} / B #{} · {} 個不同區塊（依區塊位置比對）",self.comparison_ids.0,self.comparison_ids.1,self.differences.len()));
                ui.horizontal(|ui|{
                    ui.label("查看區塊");ui.add(egui::DragValue::new(&mut self.diff_page).range(0..=left.len().max(right.len()).saturating_sub(1)));
                    if ui.button("下一處差異").clicked(){if let Some(n)=self.differences.iter().copied().find(|n|*n>self.diff_page).or_else(||self.differences.first().copied()){self.diff_page=n;}}
                });
                let a=left.get(self.diff_page).map(|s|s.as_str()).unwrap_or("");let b=right.get(self.diff_page).map(|s|s.as_str()).unwrap_or("");
                if self.diff_cache.as_ref().is_none_or(|cache|cache.0!=self.diff_page){
                    let (l,r,coarse)=crate::version_diff::diff(a,b);
                    self.diff_cache=Some((self.diff_page,crate::version_diff::layout(&l,true),crate::version_diff::layout(&r,false),coarse));
                }
                let cache=self.diff_cache.as_ref().unwrap();
                ui.label("紅底刪除線＝A 中刪除文字；綠底＝B 中新增文字；共同文字保持原色。");
                if cache.3{ui.colored_label(egui::Color32::DARK_RED,"此區塊差異過大，改為標示共同前後文之間的變更範圍。");}
                ui.columns(2,|cols|{
                    cols[0].label(format!("A #{}",self.comparison_ids.0));cols[1].label(format!("B #{}",self.comparison_ids.1));
                    egui::ScrollArea::vertical().id_salt(("compare-a",self.diff_page)).max_height(350.0).show(&mut cols[0],|ui|{ui.label(cache.1.clone());});
                    egui::ScrollArea::vertical().id_salt(("compare-b",self.diff_page)).max_height(350.0).show(&mut cols[1],|ui|{ui.label(cache.2.clone());});
                });
            }
        });self.history_open=open;
        if compare&&!self.busy(){if let Some(doc)=&self.doc{
            let a=doc.history.versions.iter().find(|v|v.id==self.left_id).map(|v|v.snapshot.clone());let b=doc.history.versions.iter().find(|v|v.id==self.right_id).map(|v|v.snapshot.clone());
            if let (Some(a),Some(b))=(a,b){let store=doc.store.clone();self.comparison_ids=(self.left_id,self.right_id);self.task(ctx,After::Nothing,move||Ok(Completion::Compare(store::read_snapshot(&store,&a)?,store::read_snapshot(&store,&b)?)));}
        }}
        if let Some((delete,id))=action{if let Some(doc)=self.doc.clone(){self.task(ctx,After::Nothing,move||if delete{store::delete(doc,id).map(Completion::Document)}else{store::restore(doc,id).map(Completion::Document)});}}
    }
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
            let _=ctx.run(Default::default(),|ctx|writer.tick(ctx));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(writer.allow_close);assert_eq!(std::fs::read_to_string(path).unwrap(),"modified");
        let doc=writer.doc.as_ref().unwrap();assert_eq!(doc.history.versions.len(),1);assert!(doc.history.draft.is_none());
        assert_eq!(doc.history.versions[0].reason,"關閉儲存");
    }
}
