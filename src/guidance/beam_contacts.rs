//! Runtime beam contacts: pairing 0x680260 / 0x67C2F0, workspace planes 0x67DA30 / 0x666340,
//! surviving intervals 0x67FA60 / 0x67F6D0 (RE/05 §5.4).
use bevy::prelude::*;
use super::{GuidanceEdge, GuidanceSubType, GuidanceWorld};
use crate::collision::CollisionWorld;

const MAX_HALF_WIDTH: f32 = 0.5; // 0x680DC0
const COS_30: f32 = 0.866_025_4;
#[derive(Clone, Copy, Debug)]
pub struct BeamContact { pub p0: Vec3, pub p1: Vec3, pub widths: [f32; 2], pub up: Vec3 }
impl BeamContact {
    pub fn point(&self, p: Vec3) -> Vec3 {
        let d = self.p1-self.p0;
        self.p0 + d * ((p-self.p0).dot(d)/d.length_squared().max(1e-12)).clamp(0.0,1.0)
    }
    pub fn width(&self, p: Vec3) -> f32 {
        let length = self.p0.distance(self.p1);
        if p.distance(self.p0) <= 0.001 { return self.widths[0]; }
        if p.distance(self.p1) <= 0.001 { return self.widths[1]; }
        let t = self.p0.distance(p)/length;
        self.widths[0] + (self.widths[1]-self.widths[0])*t
    }
    fn slice(&self, a: f32, b: f32) -> Self {
        Self { p0:self.p0.lerp(self.p1,a), p1:self.p0.lerp(self.p1,b), up:self.up, widths:[self.widths[0]+(self.widths[1]-self.widths[0])*a,self.widths[0]+(self.widths[1]-self.widths[0])*b] }
    }
}
fn wall(e: &GuidanceEdge) -> Vec3 { if e.n0.y < e.n1.y { e.n0 } else { e.n1 } }

/// Positive overlap on the mean axis and opposed normals (0x679980 / 0x67B6D0).
fn pair(a: &GuidanceEdge, b: &GuidanceEdge) -> Option<(BeamContact, f32)> {
    let (u,v) = ((a.p1-a.p0).normalize_or_zero(),(b.p1-b.p0).normalize_or_zero());
    if (a.p1-a.p0).abs().max_element()<=0.001 || (b.p1-b.p0).abs().max_element()<=0.001 || u==Vec3::ZERO || v==Vec3::ZERO || u.dot(v).abs()<COS_30 || -wall(a).dot(wall(b))<COS_30 { return None; }
    let (mut b0,mut b1,v) = if u.dot(v)<0.0 { (b.p1,b.p0,-v) } else { (b.p0,b.p1,v) };
    // 0x66C780 clips the candidate to the +/-20 degree wedge before matching.
    let inward=-wall(a).normalize_or_zero(); let across=u.cross(inward).normalize_or_zero();
    let tangent=20f32.to_radians().tan();
    let (start,end)=(b0,b1); let (mut t0,mut t1)=(0.0f32,1.0f32);
    for n in [inward,inward*tangent+across,inward*tangent-across] {
        let (x,y)=((start-a.p0).dot(n),(end-a.p0).dot(n));
        if x<0.0 && y<0.0 {return None;}
        if x<0.0 {t0=t0.max(x/(x-y));} else if y<0.0 {t1=t1.min(x/(x-y));}
    }
    if t0>t1 {return None;} b0=start.lerp(end,t0);b1=start.lerp(end,t1);
    let axis = (u+v).normalize();
    let da = u.dot(axis); let db = v.dot(axis);
    let offset=(b0-a.p0).dot(axis);
    let lo=0.0f32.max(offset); let hi=((a.p1-a.p0).dot(axis)).min((b1-a.p0).dot(axis));
    if hi<=lo { return None; }
    let at = |s:f32| a.p0+u*(s/da);
    let bt = |s:f32| b0+v*((s-offset)/db);
    let (a0,a1,b0,b1)=(at(lo),at(hi),bt(lo),bt(hi));
    let beam=BeamContact { p0:(a0+b0)*0.5,p1:(a1+b1)*0.5,widths:[a0.distance(b0)*0.5,a1.distance(b1)*0.5], up: ((if a.n0.y>=a.n1.y {a.n0}else{a.n1})+(if b.n0.y>=b.n1.y {b.n0}else{b.n1})).normalize_or(Vec3::Y) };
    let (w0,w1)=(beam.widths[0],beam.widths[1]);
    let (mut start,mut end)=(0.0,1.0);
    if w0>MAX_HALF_WIDTH+0.0005 && w1>MAX_HALF_WIDTH+0.0005 { return None; }
    if w0>MAX_HALF_WIDTH+0.0005 { if w1>=MAX_HALF_WIDTH-0.0005 { return None; } start=(MAX_HALF_WIDTH-w0)/(w1-w0); }
    if w1>MAX_HALF_WIDTH+0.0005 { if w0>=MAX_HALF_WIDTH-0.0005 { return None; } end=(MAX_HALF_WIDTH-w0)/(w1-w0); }
    let beam=beam.slice(start,end);
    (beam.p0.distance_squared(beam.p1)>0.0001).then_some((beam, (b1-a.p0).dot(u).min(a.p0.distance(a.p1))-(b0-a.p0).dot(u).max(0.0)))
}

/// Triangle/rectangle intersection used by the three native workspace planes (0x666340 / 0x97A100).
fn plane_hit(vertices:[Vec3;3], origin:Vec3, normal:Vec3, along:Vec3, across:Vec3, length:f32, width:f32) -> Option<(f32,f32,f32)> {
    let signed=vertices.map(|p|(p-origin).dot(normal));
    if signed.iter().all(|d|*d>=0.0) || signed.iter().all(|d|*d<0.0) { return None; }
    let mut points=Vec::with_capacity(2);
    for i in 0..3 { let j=(i+1)%3;
        if (signed[i]>=0.0)!=(signed[j]>=0.0) {
            points.push(vertices[i].lerp(vertices[j],signed[i]/(signed[i]-signed[j])));
        }
    }
    if points.len()!=2 { return None; }
    let p=points[0]-origin; let d=points[1]-points[0];
    let (mut lo,mut hi)=(0.0f32,1.0f32);
    for (axis,max) in [(along,length),(across,width)] {
        let (v,dv)=(p.dot(axis),d.dot(axis));
        if dv.abs()<1e-8 { if v<0.0 || v>max { return None; } }
        else { let (a,b)=(-v/dv,(max-v)/dv); lo=lo.max(a.min(b)); hi=hi.min(a.max(b)); if lo>hi{return None;} }
    }
    let (a,b)=((p+d*lo).dot(along),(p+d*hi).dot(along));
    Some((a.min(b),a.max(b),d.length()*(hi-lo)))
}

/// Accurate detector defaults 0x678F80; remove workspace-blocked intervals using 0x67F6D0's directional tags.
fn trim(beam:BeamContact, world:&CollisionWorld) -> Vec<BeamContact> {
    let axis=(beam.p1-beam.p0).normalize(); let length=beam.p0.distance(beam.p1);
    let up=beam.up; let side=(-axis.cross(up)).normalize_or_zero();
    let reach=beam.widths[0].max(beam.widths[1])+0.15;
    let left=beam.p0+up*0.5-side*reach; let right=beam.p0+up*0.5+side*reach;
    let q0=Quat::from_axis_angle(axis,-5f32.to_radians()); let q1=Quat::from_axis_angle(axis,5f32.to_radians());
    let planes=[(left,-up,side,2.0*reach),(left,q0*(-side),q0*(-up),1.0),(right,q1*side,q1*(-up),1.0)];
    let mut intervals=Vec::new();
    let mut visit=|vertices:[Vec3;3],normal:Vec3| {
        for &(origin,n,across,width) in &planes {
            if let Some((a,b,segment_length))=plane_hit(vertices,origin,n,axis,across,length,width) {
                if segment_length<=0.0005 {continue;}
                let dot=normal.dot(axis);
                let tag=if dot>std::f32::consts::FRAC_1_SQRT_2 {0} else if dot< -std::f32::consts::FRAC_1_SQRT_2 {1} else {2};
                intervals.push((a.max(0.0),b.min(length),tag));
            }
        }
    };
    // PORT: static collision meshes/boxes adapt GuidanceWorkspace's live component triangle lists.
    let min=beam.p0.min(beam.p1)-Vec3::splat(2.0); let max=beam.p0.max(beam.p1)+Vec3::splat(2.0);
    for t in world.triangles_overlapping(min,max) {visit(t.vertices,t.normal);}
    for b in &world.boxes {
        if !b.min.cmple(max).all() || !b.max.cmpge(min).all() {continue;}
        for axis in 0..3 { for high in [false,true] {
            let j=(axis+1)%3;let k=(axis+2)%3;
            let mut a=b.min;let mut c=b.min;let mut d=b.max;let mut e=b.max;
            let x=if high {b.max[axis]} else {b.min[axis]};
            for p in [&mut a,&mut c,&mut d,&mut e] {p[axis]=x;}
            c[j]=b.max[j];d[k]=b.max[k];e[j]=b.min[j];
            let mut n=Vec3::ZERO;n[axis]=if high{1.0}else{-1.0};
            visit([a,c,d],n);visit([a,d,e],n);
        }}
    }
    if intervals.is_empty() {return vec![beam];}
    intervals.sort_by(|a,b|a.0.total_cmp(&b.0));
    let mut result=Vec::new();let mut cursor=0.0;let mut tag=2;
    for (a,b,t) in intervals {
        if t!=0 && a>cursor && a-cursor>0.2 {result.push(beam.slice(cursor/length,a/length));}
        if b>cursor {cursor=b;tag=t;}
    }
    if length>cursor+0.0005 && tag!=1 && length-cursor>0.2 {result.push(beam.slice(cursor/length,1.0));}
    result
}

pub fn detect(guidance:&GuidanceWorld, world:&CollisionWorld, centre:Vec3, half:Vec3) -> Vec<BeamContact> {
    // PORT: recompute an immutable local report instead of HumanGuidance's live per-entity cached workspace.
    let mut edges:Vec<_>=guidance.edges.iter().filter(|e| e.subtype==GuidanceSubType::LedgeGrab
        && e.p0.min(e.p1).cmple(centre+half+Vec3::ONE).all() && e.p0.max(e.p1).cmpge(centre-half-Vec3::ONE).all()).cloned().collect();
    edges.sort_by(|a,b|a.p0.distance_squared(a.p1).total_cmp(&b.p0.distance_squared(b.p1)));
    let mut report=Vec::new();
    while let Some(seed)=edges.pop() {
        let candidate=edges.iter().enumerate().filter_map(|(i,e)|pair(&seed,e).map(|(b,overlap)|(i,b,overlap)))
            .max_by(|a,b|a.2.total_cmp(&b.2));
        if let Some((i,beam,_))=candidate {
            let other=edges.remove(i);
            // 0x67C2F0 returns unused portions of both contacts to the sorted input report.
            let u=(seed.p1-seed.p0).normalize(); let mut v=(other.p1-other.p0).normalize();
            if u.dot(v)<0.0 {v=-v;} let mean=(u+v).normalize();
            for e in [seed,other] {
                let delta=e.p1-e.p0;let denom=delta.dot(mean);
                let mut lo=(beam.p0-e.p0).dot(mean)/denom;let mut hi=(beam.p1-e.p0).dot(mean)/denom;
                if lo>hi {std::mem::swap(&mut lo,&mut hi);} lo=lo.clamp(0.0,1.0);hi=hi.clamp(0.0,1.0);
                for (a,b) in [(0.0,lo),(hi,1.0)] {
                    let p0=e.p0+delta*a;let p1=e.p0+delta*b;
                    if p0.distance_squared(p1)>0.0025000002 {let mut residual=e.clone();residual.p0=p0;residual.p1=p1;edges.push(residual);}
                }
            }
            edges.sort_by(|a,b|a.p0.distance_squared(a.p1).total_cmp(&b.p0.distance_squared(b.p1)));
            report.extend(trim(beam,world));
        }
    }
    // PORT: greybox-authored centre lines have no paired contact widths; retain them with zero half-width.
    if world.triangles.is_empty() {report.extend(guidance.edges.iter().filter(|e|e.subtype==GuidanceSubType::Beam).map(|e|BeamContact{p0:e.p0,p1:e.p1,widths:[0.0;2],up:e.n0}));}
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edge(a:Vec3,b:Vec3,n:Vec3)->GuidanceEdge {GuidanceEdge{p0:a,p1:b,n0:Vec3::Y,n1:n,subtype:GuidanceSubType::LedgeGrab}}
    #[test]
    fn paired_ledge_edges_produce_centre_line_and_widths() {
        let a=edge(Vec3::new(0.0,4.0,-0.2),Vec3::new(4.0,4.0,-0.2),Vec3::NEG_Z);
        let b=edge(Vec3::new(0.0,4.0,0.2),Vec3::new(4.0,4.0,0.4),Vec3::Z);
        let (beam,_)=pair(&a,&b).unwrap();assert!((beam.widths[0]-0.2).abs()<0.01);assert!((beam.widths[1]-0.3).abs()<0.01);
        assert!((beam.width((beam.p0+beam.p1)*0.5)-0.25).abs()<0.01);
        assert_eq!(beam.width(beam.p0),beam.widths[0]);
    }
    #[test]
    fn rejects_wide_perpendicular_and_same_facing_edges() {
        let a=edge(Vec3::ZERO,Vec3::X*4.0,Vec3::NEG_Z);
        let wide=edge(Vec3::Z*1.1,Vec3::Z*1.1+Vec3::X*4.0,Vec3::Z);
        assert!(pair(&a,&wide).is_none());
        let mut b=wide.clone();b.p0=Vec3::Z*0.2;b.p1=b.p0+Vec3::X*4.0;b.n1=Vec3::NEG_Z;
        assert!(pair(&a,&b).is_none());b.n1=Vec3::Z;b.p1=b.p0+Vec3::Z*4.0;assert!(pair(&a,&b).is_none());
    }
    #[test]
    fn workspace_triangle_is_clipped_to_plane_rectangle() {
        let hit=plane_hit([Vec3::new(1.0,-1.0,-1.0),Vec3::new(1.0,1.0,-1.0),Vec3::new(1.0,0.0,2.0)],Vec3::ZERO,Vec3::Y,Vec3::X,Vec3::Z,4.0,1.0).unwrap();
        assert!((hit.0-1.0).abs()<1e-6 && (hit.1-1.0).abs()<1e-6);
    }
}

#[cfg(test)]
mod accurate_tests {
    use super::*;
    fn beam()->BeamContact {BeamContact{p0:Vec3::ZERO,p1:Vec3::X*4.0,widths:[0.2;2],up:Vec3::Y}}
    #[test]
    fn transverse_wall_trims_the_correct_side_and_keeps_endpoint_widths() {
        let mut world=CollisionWorld::default();
        world.boxes.push(crate::collision::Aabb3{min:Vec3::new(2.0,-1.0,-1.0),max:Vec3::new(3.0,2.0,1.0)});
        let result=trim(beam(),&world);
        assert!(!result.is_empty());
        assert!(result.iter().all(|b|b.p0.x>=3.0 || b.p1.x<=2.0));
        assert!(result.iter().all(|b|b.widths==[0.2;2]));
    }
    #[test]
    fn contact_report_is_derived_without_authored_beam_subtype() {
        let mut guidance=GuidanceWorld::default();
        for (z,n) in [(-0.2,Vec3::NEG_Z),(0.2,Vec3::Z)] {
            guidance.edges.push(GuidanceEdge{p0:Vec3::new(0.0,0.0,z),p1:Vec3::new(4.0,0.0,z),n0:Vec3::Y,n1:n,subtype:GuidanceSubType::LedgeGrab});
        }
        let mut world=CollisionWorld::default();world.native_query_culling=true;
        let report=detect(&guidance,&world,Vec3::X*2.0,Vec3::ONE);
        assert_eq!(report.len(),1);assert_eq!(report[0].widths,[0.2;2]);
        assert_eq!(report[0].p0.z,0.0);assert_eq!(report[0].p1.z,0.0);
    }
    #[test]
    fn taper_is_clipped_at_half_metre_width() {
        let a=GuidanceEdge{p0:Vec3::ZERO,p1:Vec3::X*4.0,n0:Vec3::Y,n1:Vec3::NEG_Z,subtype:GuidanceSubType::LedgeGrab};
        let b=GuidanceEdge{p0:Vec3::Z*0.4,p1:Vec3::X*4.0+Vec3::Z*1.2,n1:Vec3::Z,..a.clone()};
        let (result,_)=pair(&a,&b).unwrap();
        assert!((result.widths[1]-0.5).abs()<0.00001);
        assert!(result.p1.x<4.0 && result.p1.x>2.0);
    }
}
