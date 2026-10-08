//! Puts Altaïr's real model (loaded from the user's install) on the player, replacing the capsule,
//! as skinned meshes driven by his movement skeleton and supplemental visual descendants.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension, TextureDataOrder};

use crate::assets::altair::load_altair;
use crate::assets::ac_formats::CHARACTER_VISUAL_FIXES;
use crate::assets::game_dir;
use crate::player::Player;

/// Marks the stand-in capsule meshes so they can be hidden once the real model is in.
#[derive(Component)]
pub struct PlaceholderBody;

#[derive(Resource, Default)]
pub struct ModelStatus(pub String);

/// The character rig: joint entities (skeleton order) and their rest local transforms.
#[derive(Component)]
pub struct Rig {
    /// Skeleton root entity (its transform maps skeleton/animation space to player space).
    pub root: Entity,
    pub joints: Vec<Entity>,
    pub rest: Vec<Transform>,
    pub bone_ids: Vec<u32>,
    pub parents: Vec<Option<usize>>,
}

pub struct ModelPlugin;

impl Plugin for ModelPlugin {
    fn build(&self, app: &mut App) {
        crate::character_material::install(app);
        app.init_resource::<ModelStatus>().add_systems(PostStartup, attach_altair)
            .add_systems(PostUpdate, crate::cloth::update_cloth.after(bevy::transform::TransformSystems::Propagate));
        // PORT: use the final Bevy leg pose for secondary targets; native task ordering remains unported.
        const ROBE_POSE_AFTER_IK: bool = true;
        let after_ik = std::env::var("AC_ROBE_POSE_AFTER_IK").map_or(ROBE_POSE_AFTER_IK, |v| v != "0");
        if after_ik {
            app.add_systems(Update, crate::visual_pose::update_rotation_copies.after(crate::ik::solve_limbs));
        } else {
            app.add_systems(Update, crate::visual_pose::update_rotation_copies.after(crate::anim::apply_clip));
        }
    }
}

/// Skeleton space (Z-up, X0-rotated) → player-local Bevy space (Y-up, facing -Z):
/// skel (x, y, z) → model (y, -x, z) → Bevy (-y, z, -x), feet lifted to y = 0.
fn skeleton_root(min_z: f32) -> Transform {
    let m = Mat3::from_cols(Vec3::new(0.0, 0.0, -1.0), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    Transform { translation: Vec3::new(0.0, -min_z, 0.0), rotation: Quat::from_mat3(&m), scale: Vec3::ONE }
}

#[allow(clippy::too_many_arguments)]
fn attach_altair(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut character_materials: ResMut<Assets<crate::character_material::CharacterSurface>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut status: ResMut<ModelStatus>,
    player: Query<Entity, With<Player>>,
    mut placeholders: Query<&mut Visibility, With<PlaceholderBody>>,
) {
    let Ok(player) = player.single() else { return };
    if std::env::var_os("AC_NO_MODEL").is_some() {
        status.0 = "model: disabled (AC_NO_MODEL)".into();
        return;
    }
    let model = match load_altair(&game_dir()) {
        Ok(m) => m,
        Err(e) => {
            status.0 = format!("model: not loaded ({e}); run once with --game-dir \"<your Assassin's Creed folder>\"");
            warn!("{}", status.0);
            return;
        }
    };

    // ---------------------------------------------------------------- skeleton → joint entities
    let root_t = skeleton_root(model.min_z);
    let root = commands.spawn((root_t, Visibility::default(), Name::new("skeleton_root"))).id();
    commands.entity(player).add_child(root);
    let n = model.skeleton.len();
    let mut joints = Vec::with_capacity(n);
    let mut rest = Vec::with_capacity(n);
    let mut global: Vec<Mat4> = Vec::with_capacity(n);
    for b in &model.skeleton {
        let t = Transform {
            translation: Vec3::from_array(b.local_pos),
            rotation: Quat::from_array(b.local_rot).normalize(),
            scale: Vec3::ONE,
        };
        let parent_global = b.parent.map(|p| global[p]).unwrap_or_else(|| root_t.to_matrix());
        global.push(parent_global * t.to_matrix());
        let e = commands.spawn((t, Visibility::default())).id();
        let parent_entity = b.parent.map(|p| joints[p]).unwrap_or(root);
        commands.entity(parent_entity).add_child(e);
        joints.push(e);
        rest.push(t);
    }
    // Supplemental visual bones inherit main joints without entering the movement Rig (RE/09 §7).
    let mut visual_joints = joints.clone();
    for b in &model.visual_bones {
        let t = Transform { translation: Vec3::from_array(b.local_pos), rotation: Quat::from_array(b.local_rot).normalize(), scale: Vec3::ONE };
        let e = commands.spawn((t, Visibility::default())).id();
        commands.entity(visual_joints[b.parent.expect("anchored visual bone")]).add_child(e);
        visual_joints.push(e);
    }
    if !model.visual_rotation_copies.is_empty() || !model.visual_compressions.is_empty() || !model.visual_look_at.is_empty() {
        commands.entity(player).insert(crate::visual_pose::VisualRotationCopies {
            joints: visual_joints.clone(),
            parents: model.skeleton.iter().chain(&model.visual_bones).map(|b| b.parent).collect(),
            copies: model.visual_rotation_copies.clone(),
            compressions: model.visual_compressions.clone(),
            look_at: model.visual_look_at.clone(),
            hinges: model.visual_hinges.clone(),
            hinge_states: vec![crate::skirt_hinge::HingeState::default(); model.visual_hinges.len()],
            root,
            previous_anchor: None,
        });
    }
    let inv: Vec<Mat4> = global.iter().map(|g| g.inverse()).collect();
    let skinned = !joints.is_empty();
    let inv_handle = bindposes.add(SkinnedMeshInverseBindposes::from(inv));

    // ---------------------------------------------------------------- textures
    let mut tex_handles = std::collections::HashMap::new();
    for (id, t) in model.textures.iter().chain(model.normal_textures.iter()) {
        let mut img = Image::new_uninit(
            Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            if model.normal_textures.contains_key(id) { TextureFormat::Rgba8Unorm } else { TextureFormat::Rgba8UnormSrgb },
            RenderAssetUsages::RENDER_WORLD,
        );
        img.texture_descriptor.mip_level_count = t.mips.len() as u32;
        img.data = Some(t.mips.concat());
        if CHARACTER_VISUAL_FIXES {
            // The outfit UVs tile beyond [0, 1]; clamp sampling smears the atlas border (RE/09 §6).
            img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            });
        }
        tex_handles.insert(*id, images.add(img));
    }
    let untextured = materials.add(StandardMaterial {
        base_color: Color::srgb(0.85, 0.85, 0.82),
        perceptual_roughness: 0.9,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    // Separate handles retain linear data sampling even when skin uses its diffuse map as a mask.
    let mut material_handles = std::collections::HashMap::new();
    for (id, t) in &model.material_textures {
        let mut image = Image::new_uninit(Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
            TextureDimension::D2, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
        image.texture_descriptor.mip_level_count = t.mips.len() as u32;
        image.data = Some(t.mips.concat());
        let ramp = model.parts.iter().flat_map(|p| &p.materials).any(|m| m.ramp == Some(*id));
        let mode = if ramp { ImageAddressMode::ClampToEdge } else { ImageAddressMode::Repeat };
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { address_mode_u: mode, address_mode_v: mode, ..ImageSamplerDescriptor::linear() });
        material_handles.insert(*id, images.add(image));
    }
    let mut cube_handles = std::collections::HashMap::new();
    for (id, t) in &model.cube_textures {
        let mut image = Image::new_uninit(Extent3d { width: t.size, height: t.size, depth_or_array_layers: 6 },
            TextureDimension::D2, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
        image.texture_descriptor.mip_level_count = t.mips.len() as u32;
        image.data = Some(t.mips.concat());
        image.data_order = TextureDataOrder::MipMajor;
        image.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
        cube_handles.insert(*id, images.add(image));
    }

    // ---------------------------------------------------------------- meshes
    let mut tris = 0;
    for part in &model.parts {
        let (part_inv, part_joints) = if CHARACTER_VISUAL_FIXES {
            (bindposes.add(SkinnedMeshInverseBindposes::from(part.inverse_bindposes.iter().map(Mat4::from_cols_array).collect::<Vec<_>>())),
             part.skin_joints.iter().map(|&j| visual_joints[j]).collect::<Vec<_>>())
        } else { (inv_handle.clone(), joints.clone()) };
        for ((((idx, tex), normal), authored), inside) in part.sections.iter().zip(&part.normal_maps).zip(&part.materials).zip(&part.inside_materials) {
            tris += idx.len() / 3;
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, part.tangents.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs.clone());
            if skinned && part.cloth.is_none() {
                mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(part.joints.clone()));
                mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, part.weights.clone());
            }
            mesh.insert_indices(Indices::U32(idx.clone()));
            let mat = match tex.and_then(|t| tex_handles.get(&t)) {
                // Cloth (robe, flaps, hood) is single-layer geometry the game draws two-sided; with back-face
                // culling the inner side disappears and the body shows through. BC1 textures carry 1-bit alpha for
                // frayed edges and feathers: alpha-tested.
                Some(h) => materials.add(StandardMaterial {
                    base_color_texture: Some(h.clone()),
                    normal_map_texture: if CHARACTER_VISUAL_FIXES { normal.and_then(|id| tex_handles.get(&id)).cloned() } else { None },
                    // PORT: DirectX normal-map convention; the game's shader lighting is not yet reproduced (RE/09 §6).
                    flip_normal_map_y: true,
                    perceptual_roughness: 0.85,
                    double_sided: true,
                    cull_mode: None,
                    alpha_mode: AlphaMode::Mask(0.5),
                    ..default()
                }),
                None => untextured.clone(),
            };
            let mesh_handle = meshes.add(mesh);
            let mut ec = commands.spawn((Mesh3d(mesh_handle.clone()), Transform::default(), Name::new(part.name.clone())));
            if crate::character_material::enabled() {
                let mut base = materials.get(&mat).expect("inserted character material").clone();
                if authored.style().expect("validated template") == 4 { base.alpha_mode = AlphaMode::Opaque; }
                if inside.is_some() { base.cull_mode = Some(bevy::render::render_resource::Face::Back); }
                base.base_color_texture = authored.diffuse_map.and_then(|id| tex_handles.get(&id)).cloned();
                base.normal_map_texture = authored.normal_map.and_then(|id| tex_handles.get(&id)).cloned();
                let extension = crate::character_material::CharacterLayers {
                    controls: crate::character_material::controls(authored),
                    specular_map: authored.specular_map.and_then(|id| material_handles.get(&id)).cloned(),
                    ramp: authored.ramp.and_then(|id| material_handles.get(&id)).cloned(),
                    multiply_map: authored.multiply_map.and_then(|id| material_handles.get(&id)).cloned(),
                    eye_cube: authored.eye_cube.and_then(|id| cube_handles.get(&id)).cloned(),
                };
                let material = character_materials.add(crate::character_material::CharacterSurface { base, extension });
                ec.insert(MeshMaterial3d(material));
            } else { ec.insert(MeshMaterial3d(mat)); }
            if let Some(settings) = &part.cloth {
                ec.insert(crate::cloth::CharacterCloth {
                    settings: settings.clone(), rest: part.positions.iter().map(|p| Vec3::from_array(*p)).collect(),
                    weights: part.weights.clone(), palette: part.joints.clone(),
                    inverse_bindposes: part.inverse_bindposes.iter().map(Mat4::from_cols_array).collect(),
                    joints: part_joints.clone(), collider_joints: settings.colliders.iter().map(|c| {
                        let index = model.skeleton.iter().position(|b| b.bone_id == c.bone_id).expect("validated cloth bone");
                        joints[index]
                    }).collect(), mesh: mesh_handle.clone(), player, state: default(),
                });
            } else if skinned {
                ec.insert(SkinnedMesh { inverse_bindposes: part_inv.clone(), joints: part_joints.clone() });
            }
            let child = ec.id();
            commands.entity(player).add_child(child);
            if crate::character_material::enabled() {
                if let Some(inside) = inside {
                    // The authored inside material has its own diffuse/normal/specular controls.
                    // PORT: a second culled draw replaces native inside-material dispatch (RE/09 §9).
                    let base = StandardMaterial {
                        base_color_texture: inside.diffuse_map.and_then(|id| tex_handles.get(&id)).cloned(),
                        normal_map_texture: inside.normal_map.and_then(|id| tex_handles.get(&id)).cloned(),
                        flip_normal_map_y: true, double_sided: true,
                        cull_mode: Some(bevy::render::render_resource::Face::Front),
                        alpha_mode: AlphaMode::Mask(0.5), ..default()
                    };
                    let extension = crate::character_material::CharacterLayers {
                        controls: crate::character_material::controls(inside),
                        specular_map: inside.specular_map.and_then(|id| material_handles.get(&id)).cloned(),
                        ramp: inside.ramp.and_then(|id| material_handles.get(&id)).cloned(),
                        multiply_map: inside.multiply_map.and_then(|id| material_handles.get(&id)).cloned(),
                        eye_cube: None,
                    };
                    let material = character_materials.add(crate::character_material::CharacterSurface { base, extension });
                    let mut back = commands.spawn((Mesh3d(mesh_handle.clone()), MeshMaterial3d(material), Transform::default(), Name::new(format!("{} inside", part.name))));
                    // Both draws share the simulated cloth mesh; do not run a second solver.
                    if skinned && part.cloth.is_none() {
                        back.insert(SkinnedMesh { inverse_bindposes: part_inv.clone(), joints: part_joints.clone() });
                    }
                    let child = back.id();
                    commands.entity(player).add_child(child);
                }
            }
        }
    }
    for mut v in &mut placeholders {
        *v = Visibility::Hidden;
    }
    commands.entity(player).insert((crate::anim::AnimPlayer::default(), Rig {
        root,
        joints,
        rest,
        bone_ids: model.skeleton.iter().map(|b| b.bone_id).collect(),
        parents: model.skeleton.iter().map(|b| b.parent).collect(),
    }));
    status.0 = format!(
        "model: Altair from your install ({} parts, {} tris, {} textures, {} movement + {} visual bones)",
        model.parts.len(),
        tris,
        model.textures.len(),
        n,
        model.visual_bones.len()
    );
    info!("{} — {}", status.0, model.source);
}
