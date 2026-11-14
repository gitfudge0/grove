//! Publish presentation examples through live Codex in an explicitly isolated QA configuration.
use grove_core::{
    provider::{Provider, SavedSession},
    visuals::{self, Input},
};
use serde_json::json;
use std::time::{Duration, Instant};
fn publish_live(saved: SavedSession, args: serde_json::Value, title: &str) -> Result<(), String> {
    use grove_core::codex::{self, Action, Status};
    let c = codex::connect(saved, Vec::new());
    let result = (|| {
        let deadline = Instant::now() + Duration::from_mins(3);
        loop {
            let snapshot = c
                .updates
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| e.to_string())?;
            if matches!(snapshot.status, Status::Failed | Status::Disconnected) {
                return Err(format!("Provider unavailable: {:?}", snapshot.error));
            }
            if snapshot.status == Status::Idle {
                break;
            }
        }
        c.handle.send(Action::Prompt(format!("Publish this offline QA example. Do not read files, run commands, delegate, or change the provided HTML. Discover grove_visuals.render_html and call it exactly once with this JSON argument object: {args}. Do not call preview_html (this generator has no native UI event loop). Finish with one plain sentence; do not echo the HTML.")))?;
        let mut saved = loop {
            let snapshot = c
                .updates
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| e.to_string())?;
            if !snapshot.approvals.is_empty() {
                return Err("Unexpected approval in QA publication".into());
            }
            if matches!(snapshot.status, Status::Failed | Status::Disconnected) {
                return Err(format!("Provider failed: {:?}", snapshot.error));
            }
            if snapshot.status == Status::Done {
                break snapshot.saved;
            }
        };
        if saved.visuals.len() != 1
            || saved.visuals[0].presentation.mode
                != serde_json::from_value(args["presentation"].clone())
                    .map_err(|e| e.to_string())?
        {
            return Err("Provider did not publish the requested presentation".into());
        }
        c.handle
            .send(Action::SetTitle(format!("HTML QA · {title}")))?;
        loop {
            let snapshot = c
                .updates
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| e.to_string())?;
            if snapshot.saved.title.as_deref() == Some(format!("HTML QA · {title}").as_str()) {
                saved = snapshot.saved;
                break;
            }
        }
        saved.persist_recovery()?;
        println!("Published live example: {title}");
        Ok(())
    })();
    let _ = c
        .handle
        .stop_completion(false)
        .recv_timeout(Duration::from_secs(10));
    result
}

use std::fmt::Write as _;
fn main() -> Result<(), String> {
    if std::env::var("GROVE_VISUAL_GALLERY_QA").as_deref() != Ok("1") {
        return Err("Set GROVE_VISUAL_GALLERY_QA=1 and a disposable GROVE_CONFIG_DIR".into());
    }
    let config = std::path::PathBuf::from(
        std::env::var("GROVE_CONFIG_DIR").map_err(|_| "Missing isolated GROVE_CONFIG_DIR")?,
    );
    let cwd = config.join("visual-gallery-cwd");
    fs_err::create_dir_all(&cwd).map_err(|e| e.to_string())?;
    let common = "<style>*{box-sizing:border-box}body{font:var(--grove-body) var(--grove-font);padding:20px}h1{font-size:var(--grove-title);margin:0 0 8px}h2{font-size:20px}.muted{color:var(--grove-muted)}.card{background:var(--grove-surface);border:1px solid var(--grove-border);border-radius:var(--grove-radius);padding:18px}button{font:inherit;background:var(--grove-info);border:0;border-radius:8px;padding:8px 12px;color:var(--grove-bg);cursor:pointer}input{font:inherit;color:var(--grove-fg);background:var(--grove-bg);border:1px solid var(--grove-border);border-radius:8px;padding:8px;max-width:100%}</style>";
    let dashboard = r#"<style>.metrics{display:grid;grid-template-columns:repeat(3,1fr);gap:16px;margin:20px 0}.metric{font-size:32px;font-weight:700}.bars{height:180px;display:flex;align-items:end;gap:20px;margin-top:20px}.bar{flex:1;background:var(--grove-info);border-radius:8px 8px 0 0}.controls{display:flex;gap:12px;align-items:center;margin-top:20px}@media(max-width:600px){.metrics{grid-template-columns:1fr}.controls{flex-wrap:wrap}}</style><h1>Workspace pulse</h1><div class=muted>Desktop layout · expand for responsive interaction</div><div class=metrics><div class=card>Visits<div class=metric>1,248</div></div><div class=card>Tasks<div class=metric>86</div></div><div class=card>Uptime<div class=metric>99.9%</div></div></div><div class=card><h2>Activity this week</h2><div class=bars><div class=bar style="height:35%"></div><div class=bar style="height:58%"></div><div class=bar style="height:46%"></div><div class=bar style="height:80%"></div><div class=bar style="height:95%"></div></div></div><div class="card controls"><input value="Edit and retain me"><button onclick="this.textContent='Count '+(++this.dataset.count)" data-count=0>Count 0</button></div>"#;
    let canvas = r#"<style>html,body{height:100%;overflow:hidden}body{padding:0}.tools{position:absolute;z-index:2;left:16px;top:16px;display:flex;flex-wrap:wrap;max-width:calc(100% - 32px);gap:8px;align-items:center;background:var(--grove-surface);padding:12px;border-radius:12px}#stage{position:absolute;inset:0;overflow:hidden;touch-action:none;cursor:grab;background-image:radial-gradient(var(--grove-border) 1px,transparent 1px);background-size:20px 20px}#world{transform-origin:0 0;position:absolute;width:3200px;height:2000px}.frame{position:absolute;width:420px;height:300px;border:2px solid var(--grove-info);border-radius:14px;padding:28px;background:var(--grove-surface)}.frame h2{margin-top:0}.wire{height:18px;margin:18px 0;background:var(--grove-border);border-radius:6px}</style><div id=stage><div id=world><div class=frame style="left:800px;top:500px"><h2>01 · Welcome</h2><p class=muted>Selected canvas frame</p><div class=wire></div><div class=wire style="width:65%"></div><input value="Canvas note"></div><div class=frame style="left:1320px;top:500px"><h2>02 · Workspace</h2><div class=wire></div><div class=wire></div><button>Continue →</button></div><div class=frame style="left:800px;top:920px"><h2>03 · Insights</h2><p>Pan to explore this frame</p></div></div></div><div class=tools><strong>Canvas</strong><button id=minus>−</button><span id=zoom></span><button id=plus>+</button><button id=fit>Fit selection</button><span class=muted>Drag background to pan</span></div><script>const stage=document.getElementById('stage'),world=document.getElementById('world'),label=document.getElementById('zoom');let camera={x:0,y:0,z:1},frame={x:760,y:450,width:1040,height:420};function draw(){world.style.transform=`translate(${camera.x}px,${camera.y}px) scale(${camera.z})`;label.textContent=Math.round(camera.z*100)+'%'}function fit(){const r=stage.getBoundingClientRect();camera.z=Math.min((r.width-64)/frame.width,(r.height-110)/frame.height,1);camera.x=(r.width-frame.width*camera.z)/2-frame.x*camera.z;camera.y=80+(r.height-110-frame.height*camera.z)/2-frame.y*camera.z;draw()}addEventListener('grove:initial-view',e=>{if(e.detail.frame)frame=e.detail.frame;fit()});document.getElementById('fit').onclick=fit;function scale(f){const r=stage.getBoundingClientRect(),x=r.width/2,y=r.height/2,n=Math.max(.1,Math.min(3,camera.z*f));camera.x=x-(x-camera.x)*n/camera.z;camera.y=y-(y-camera.y)*n/camera.z;camera.z=n;draw()}document.getElementById('plus').onclick=()=>scale(1.2);document.getElementById('minus').onclick=()=>scale(1/1.2);let drag=null;stage.onpointerdown=e=>{if(e.target.closest('input,button'))return;drag={x:e.clientX,y:e.clientY,cx:camera.x,cy:camera.y};stage.setPointerCapture(e.pointerId)};stage.onpointermove=e=>{if(drag){camera.x=drag.cx+e.clientX-drag.x;camera.y=drag.cy+e.clientY-drag.y;draw()}};stage.onpointerup=()=>drag=null;draw();</script>"#;
    let mut report = "<h1>Quarterly field notes</h1><p class=muted>Opening-section preview · expand to read the complete report</p>".to_string();
    for i in 1..=12 {
        let _ = write!(report,"<section class=card style='margin-top:20px'><h2>{i:02} · Research findings</h2><p>Teams need room to inspect a document without stretching the conversation. This section represents a full report, with readable paragraphs and normal document scrolling.</p><p>Publication stores the document separately from its compact transcript preview.</p></section>");
    }
    let chart = "<h2>Build health</h2><div style='display:flex;gap:12px;align-items:center'><div style='height:20px;width:70%;background:var(--grove-success);border-radius:6px'></div><strong>92%</strong></div><p class=muted>Small visual stays live in the conversation.</p><button onclick=\"this.textContent='Checked '+(++this.dataset.n)\" data-n=0>Check 0</button>";
    let mut workers = Vec::new();
    for (mode, title, body, region) in [
        ("inline", "01 · Small live chart", chart, None),
        ("dashboard", "02 · Dashboard preview", dashboard, None),
        (
            "canvas",
            "03 · Canvas preview",
            canvas,
            Some(json!({"x":760,"y":450,"width":1040,"height":420})),
        ),
        (
            "document",
            "04 · Long report preview",
            report.as_str(),
            None,
        ),
    ] {
        let key = grove_core::identity::random_id()?;
        let mut saved = SavedSession::empty(
            key.clone(),
            Provider::Codex,
            String::new(),
            cwd.display().to_string(),
        );
        saved.title = Some(format!("HTML QA · {title}"));
        let source =
            format!("<!doctype html><html><head>{common}</head><body>{body}</body></html>");
        let args = json!({"request_id":key,"title":title,"html":source,"presentation":mode,"preview_region":region});
        // Validate locally before asking the provider to publish the identical arguments.
        let input: Input = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        visuals::prepare(&input, &cwd, &[], false)?;
        workers.push(std::thread::spawn(move || publish_live(saved, args, title)));
    }
    for worker in workers {
        worker.join().map_err(|_| "QA provider worker stopped")??;
    }
    Ok(())
}
