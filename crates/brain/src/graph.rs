//! Self-contained HTML export of the brain link graph (no external
//! resources; force-directed layout on a `<canvas>` in inline JS).

use crate::error::Result;
use crate::index::Brain;
use serde_json::json;

const TEMPLATE: &str = r##"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>TAG brain graph</title>
<style>
:root{--bg:#10131a;--fg:#d8dee9;--mut:#7b869a;--panel:#171c26;--line:#2a3142}
@media (prefers-color-scheme: light){:root{--bg:#f6f7fa;--fg:#1d2330;--mut:#667085;--panel:#fff;--line:#d5d9e2}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.45 system-ui,sans-serif;display:flex;height:100vh;overflow:hidden}
#main{flex:1;position:relative;min-width:0}
canvas{width:100%;height:100%;display:block;cursor:grab}
#legend{position:absolute;top:10px;left:10px;background:var(--panel);border:1px solid var(--line);border-radius:6px;padding:8px 10px;max-width:260px;font-size:12px}
#legend h4{margin:6px 0 4px;font-size:11px;text-transform:uppercase;color:var(--mut)}
#legend h4:first-child{margin-top:0}
.sw{display:inline-block;width:10px;height:10px;border-radius:50%;margin-right:6px}
.pj{display:block;margin:2px 0}
#panel{width:340px;max-width:45vw;border-left:1px solid var(--line);background:var(--panel);padding:14px;overflow:auto}
#panel h2{margin:0 0 4px;font-size:16px}
#panel .meta{color:var(--mut);font-size:12px;margin-bottom:10px}
#panel pre{white-space:pre-wrap;word-wrap:break-word;font:12px/1.5 ui-monospace,monospace}
@media (max-width:700px){body{flex-direction:column}#panel{width:100%;max-width:none;height:40vh;border-left:0;border-top:1px solid var(--line)}}
</style></head><body>
<div id="main"><canvas id="c"></canvas><div id="legend"></div></div>
<div id="panel"><h2 id="pt">Click a node</h2><div class="meta" id="pm"></div><pre id="pb"></pre></div>
<script id="data" type="application/json">__DATA__</script>
<script>
(function(){
var G=JSON.parse(document.getElementById('data').textContent);
var KC={brief:'#e5c07b',note:'#61afef',decision:'#e06c75',pattern:'#98c379'};
var PAL=['#c678dd','#56b6c2','#d19a66','#be5046','#7f9cf5','#8bd5ca','#f5a97f','#a6da95'];
var cv=document.getElementById('c'),ctx=cv.getContext('2d');
var W=0,H=0,dpr=window.devicePixelRatio||1;
function resize(){var r=cv.getBoundingClientRect();W=r.width;H=r.height;cv.width=W*dpr;cv.height=H*dpr;ctx.setTransform(dpr,0,0,dpr,0,0);}
window.addEventListener('resize',resize);resize();
var projects=[],pidx={};
G.nodes.forEach(function(n){var p=n.project||'(global)';n.pj=p;if(!(p in pidx)){pidx[p]=projects.length;projects.push(p);}});
var nodes=G.nodes,byId={};
nodes.forEach(function(n,i){
  var k=pidx[n.pj],a=2*Math.PI*k/Math.max(projects.length,1);
  var R=projects.length>1?Math.min(W,H)*0.28:0;
  n.cx=Math.cos(a)*R;n.cy=Math.sin(a)*R;
  n.x=n.cx+(Math.random()-.5)*80;n.y=n.cy+(Math.random()-.5)*80;n.vx=0;n.vy=0;byId[n.id]=n;});
var edges=G.edges.filter(function(e){return byId[e[0]]&&byId[e[1]];}).map(function(e){return {a:byId[e[0]],b:byId[e[1]]};});
var legend=document.getElementById('legend'),h='<h4>Kind</h4>';
Object.keys(KC).forEach(function(k){h+='<span class="pj"><span class="sw" style="background:'+KC[k]+'"></span>'+k+'</span>';});
h+='<h4>Project cluster</h4>';
projects.forEach(function(p,i){h+='<span class="pj"><span class="sw" style="border:2px solid '+PAL[i%PAL.length]+'"></span><span class="pl"></span></span>';});
legend.innerHTML=h;
var pls=legend.querySelectorAll('.pl');projects.forEach(function(p,i){pls[i].textContent=p;});
var sel=null,tx=0,ty=0,scale=1,alpha=1;
function step(){
  var i,j,n,m,dx,dy,d2,d,f;
  for(i=0;i<nodes.length;i++){n=nodes[i];
    for(j=i+1;j<nodes.length;j++){m=nodes[j];dx=n.x-m.x;dy=n.y-m.y;d2=dx*dx+dy*dy+0.01;
      if(d2<90000){d=Math.sqrt(d2);f=1800/d2;n.vx+=dx/d*f;n.vy+=dy/d*f;m.vx-=dx/d*f;m.vy-=dy/d*f;}}
    n.vx+=(n.cx-n.x)*0.004;n.vy+=(n.cy-n.y)*0.004;}
  edges.forEach(function(e){dx=e.b.x-e.a.x;dy=e.b.y-e.a.y;d=Math.sqrt(dx*dx+dy*dy)+0.01;f=(d-70)*0.02;
    e.a.vx+=dx/d*f;e.a.vy+=dy/d*f;e.b.vx-=dx/d*f;e.b.vy-=dy/d*f;});
  nodes.forEach(function(n){if(n.drag)return;n.vx*=0.82;n.vy*=0.82;n.x+=n.vx*alpha;n.y+=n.vy*alpha;});
  alpha=Math.max(alpha*0.995,0.05);
}
function draw(){
  ctx.clearRect(0,0,W,H);ctx.save();ctx.translate(W/2+tx,H/2+ty);ctx.scale(scale,scale);
  ctx.lineWidth=1/scale;ctx.strokeStyle='rgba(128,140,160,.45)';
  edges.forEach(function(e){ctx.beginPath();ctx.moveTo(e.a.x,e.a.y);ctx.lineTo(e.b.x,e.b.y);ctx.stroke();});
  nodes.forEach(function(n){
    ctx.beginPath();ctx.arc(n.x,n.y,n===sel?8:6,0,6.2832);ctx.fillStyle=KC[n.kind]||'#999';ctx.fill();
    ctx.lineWidth=2/scale;ctx.strokeStyle=PAL[pidx[n.pj]%PAL.length];ctx.stroke();
    if(n===sel||scale>1.4){ctx.fillStyle=getComputedStyle(document.body).color;ctx.font=(11/scale)+'px sans-serif';ctx.fillText(n.title,n.x+9/scale,n.y+4/scale);}
  });
  ctx.restore();
}
function loop(){step();draw();requestAnimationFrame(loop);}
function toWorld(ev){var r=cv.getBoundingClientRect();return {x:(ev.clientX-r.left-W/2-tx)/scale,y:(ev.clientY-r.top-H/2-ty)/scale};}
function pick(ev){var p=toWorld(ev),best=null,bd=14/scale;nodes.forEach(function(n){var d=Math.hypot(n.x-p.x,n.y-p.y);if(d<bd){bd=d;best=n;}});return best;}
var drag=null,pan=null,moved=false;
cv.addEventListener('mousedown',function(ev){moved=false;var n=pick(ev);if(n){drag=n;n.drag=true;}else{pan={x:ev.clientX-tx,y:ev.clientY-ty};}});
window.addEventListener('mousemove',function(ev){
  if(drag){var p=toWorld(ev);drag.x=p.x;drag.y=p.y;drag.vx=drag.vy=0;moved=true;alpha=0.5;}
  else if(pan){tx=ev.clientX-pan.x;ty=ev.clientY-pan.y;moved=true;}});
window.addEventListener('mouseup',function(ev){
  if(drag){drag.drag=false;}
  if(!moved){var n=pick(ev);if(n)show(n);}
  drag=null;pan=null;});
cv.addEventListener('wheel',function(ev){ev.preventDefault();scale=Math.min(4,Math.max(0.3,scale*(ev.deltaY<0?1.1:0.9)));},{passive:false});
function show(n){
  sel=n;
  document.getElementById('pt').textContent=n.title;
  document.getElementById('pm').textContent=n.kind+' · '+(n.project||'global')+' · '+n.id;
  document.getElementById('pb').textContent=n.body||'';
}
loop();
})();
</script></body></html>
"##;

/// Escape a JSON string so it can sit inside a `<script>` element and so the
/// page never contains a literal `://` (document bodies may hold URLs).
fn script_safe(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
        .replace("://", ":\\/\\/")
}

/// Build graph HTML from raw node/edge data (see [`graph_html`]).
pub fn render_graph_html(nodes: &[serde_json::Value], edges: &[(String, String)]) -> String {
    let data = json!({ "nodes": nodes, "edges": edges });
    let json =
        serde_json::to_string(&data).unwrap_or_else(|_| "{\"nodes\":[],\"edges\":[]}".into());
    TEMPLATE.replace("__DATA__", &script_safe(&json))
}

/// Export the whole brain (or one project plus global docs when `project`
/// is given) as a self-contained HTML page.
pub fn graph_html(brain: &Brain, project: Option<&str>) -> Result<String> {
    let mut nodes = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for m in brain.all_docs()? {
        if let Some(p) = project {
            if m.project.as_deref().is_some_and(|mp| mp != p) {
                continue;
            }
        }
        let body = brain.get(&m.id)?.map(|d| d.body).unwrap_or_default();
        ids.insert(m.id.clone());
        nodes.push(json!({
            "id": m.id,
            "kind": m.kind.as_str(),
            "title": m.title,
            "project": m.project,
            "body": body,
        }));
    }
    let edges: Vec<(String, String)> = brain
        .links()?
        .into_iter()
        .filter(|(a, b)| ids.contains(a) && ids.contains(b))
        .collect();
    Ok(render_graph_html(&nodes, &edges))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_external_urls_and_script_safe() {
        let nodes = vec![json!({
            "id": "n1", "kind": "note", "title": "<b>x</b>", "project": null,
            "body": "</script><script>alert(1)</script> see https://example.com and http://a.b",
        })];
        let html = render_graph_html(&nodes, &[("n1".into(), "n1".into())]);
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
        assert_eq!(
            html.matches("</script>").count(),
            2,
            "only our own two script closers"
        );
        assert!(!html.contains("<b>x</b>"));
        assert!(html.contains("<canvas"));
    }

    #[test]
    fn graph_from_brain() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("b");
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(
            root.join("notes/a.md"),
            "---\ntitle: A\n---\nlinks [[note_b]]\n",
        )
        .unwrap();
        std::fs::write(root.join("notes/b.md"), "---\ntitle: B\n---\nbody\n").unwrap();
        let mut brain = Brain::open(
            &t.path().join("i.db"),
            vec![crate::index::BrainRoot {
                path: root,
                project: Some("p1".into()),
            }],
        )
        .unwrap();
        brain.reindex().unwrap();
        let html = graph_html(&brain, Some("p1")).unwrap();
        assert!(html.contains("note_a") && html.contains("note_b"));
        assert!(!html.contains("http://") && !html.contains("https://"));
        let none = graph_html(&brain, Some("other")).unwrap();
        assert!(!none.contains("\"note_a\""));
    }
}
