use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_RESOURCE_COUNT: usize = 4096;
pub const MAX_DATA_URI_BYTES: usize = 64 * 1024 * 1024;
// Counts remain comfortably below JavaScript's exact-integer ceiling and bound downstream work.
pub const MAX_ACCESSOR_COUNT: u64 = 10_000_000;
pub const MAX_AGGREGATE_VERTICES: u64 = 10_000_000;
pub const MAX_AGGREGATE_TRIANGLES: u64 = 10_000_000;
const MAX_AGGREGATE_PRIMITIVES: u64 = 1_000_000;
const JSON_CHUNK: u32 = 0x4e4f534a;
const BIN_CHUNK: u32 = 0x004e4942;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Glb,
    Gltf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model3dMetadata {
    pub vertex_count: u64,
    pub triangle_count: u64,
    pub mesh_count: u32,
    pub primitive_count: u32,
    pub material_count: u32,
    pub texture_count: u32,
    pub animation_count: u32,
    pub has_skin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds_min: Option<[f64; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds_max: Option<[f64; 3]>,
    pub gltf_version: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedModel {
    pub metadata: Model3dMetadata,
    pub validation_level: &'static str,
    pub container: &'static str,
}

#[derive(Debug, Error, PartialEq)]
pub enum ModelValidationError {
    #[error("invalid model3d data: {0}")]
    Invalid(String),
}

pub fn validate(bytes: &[u8], format: ModelFormat) -> Result<ValidatedModel, ModelValidationError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_MODEL_BYTES {
        return invalid("file is empty or exceeds 256 MiB");
    }
    let (json, bin, level, container) = match format {
        ModelFormat::Glb => {
            let (json, bin) = parse_glb(bytes)?;
            (json, bin, "structural", "GLB")
        }
        ModelFormat::Gltf => (
            serde_json::from_slice(bytes).map_err(|_| error("GLTF is not valid UTF-8 JSON"))?,
            None,
            "structural-self-contained",
            "GLTF",
        ),
    };
    let metadata = validate_document(&json, bin, format)?;
    Ok(ValidatedModel {
        metadata,
        validation_level: level,
        container,
    })
}

fn parse_glb(bytes: &[u8]) -> Result<(Value, Option<&[u8]>), ModelValidationError> {
    if bytes.len() < 20 || &bytes[..4] != b"glTF" {
        return invalid("GLB magic is not glTF");
    }
    if u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != 2 {
        return invalid("GLB version must be 2");
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize != bytes.len() {
        return invalid("GLB declared length does not match file length");
    }
    let mut offset = 12usize;
    let mut json = None;
    let mut bin = None;
    let mut chunk_index = 0;
    while offset < bytes.len() {
        if !offset.is_multiple_of(4) || bytes.len() - offset < 8 {
            return invalid("GLB chunk header is truncated or unaligned");
        }
        let length = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        if !length.is_multiple_of(4) {
            return invalid("GLB chunk length is not 4-byte aligned");
        }
        let start = offset
            .checked_add(8)
            .ok_or_else(|| error("GLB chunk overflow"))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| error("GLB chunk overflow"))?;
        if end > bytes.len() {
            return invalid("GLB chunk is truncated");
        }
        if chunk_index == 0 && kind != JSON_CHUNK {
            return invalid("GLB JSON chunk must be first");
        }
        match kind {
            JSON_CHUNK if json.is_none() => {
                let chunk = &bytes[start..end];
                let json_end = chunk
                    .iter()
                    .rposition(|byte| *byte != b' ')
                    .map(|index| index + 1)
                    .ok_or_else(|| error("GLB JSON chunk is empty"))?;
                if chunk[json_end - 1] != b'}' || chunk[json_end..].iter().any(|byte| *byte != b' ')
                {
                    return invalid("GLB JSON padding must contain spaces only");
                }
                let text = std::str::from_utf8(&chunk[..json_end])
                    .map_err(|_| error("GLB JSON chunk is not UTF-8"))?;
                json = Some(serde_json::from_str(text).map_err(|_| error("GLB JSON is invalid"))?);
            }
            JSON_CHUNK => return invalid("GLB has multiple JSON chunks"),
            BIN_CHUNK if bin.is_none() => bin = Some(&bytes[start..end]),
            BIN_CHUNK => return invalid("GLB has multiple BIN chunks"),
            // The controlled foundation profile deliberately rejects unknown chunks rather than
            // silently accepting data no downstream structural consumer understands.
            _ => return invalid("GLB contains an unsupported chunk type"),
        }
        offset = end;
        chunk_index += 1;
    }
    Ok((json.ok_or_else(|| error("GLB has no JSON chunk"))?, bin))
}

fn validate_document(
    root: &Value,
    bin: Option<&[u8]>,
    format: ModelFormat,
) -> Result<Model3dMetadata, ModelValidationError> {
    let object = root
        .as_object()
        .ok_or_else(|| error("glTF root must be an object"))?;
    if object
        .get("asset")
        .and_then(|v| v.get("version"))
        .and_then(Value::as_str)
        != Some("2.0")
    {
        return invalid("asset.version must be 2.0");
    }
    validate_required_extensions(object.get("extensionsRequired"))?;

    let buffers = array(root, "buffers")?;
    let views = array(root, "bufferViews")?;
    let accessors = array(root, "accessors")?;
    let meshes = array(root, "meshes")?;
    let nodes = array(root, "nodes")?;
    let materials = array(root, "materials")?;
    let textures = array(root, "textures")?;
    let images = array(root, "images")?;
    let animations = array(root, "animations")?;
    let skins = array(root, "skins")?;
    let scenes = array(root, "scenes")?;
    let texture_samplers = array(root, "samplers")?;
    for values in [
        &buffers,
        &views,
        &accessors,
        &meshes,
        &nodes,
        &materials,
        &textures,
        &images,
        &animations,
        &skins,
        &scenes,
        &texture_samplers,
    ] {
        if values.len() > MAX_RESOURCE_COUNT {
            return invalid("resource count exceeds 4096");
        }
    }

    let mut buffer_lengths = Vec::with_capacity(buffers.len());
    let mut bin_consumed = false;
    for (index, buffer) in buffers.iter().enumerate() {
        let declared = uint(buffer, "byteLength")?;
        if declared == 0 {
            return invalid("buffer byteLength must be positive");
        }
        let uri = buffer.get("uri").and_then(Value::as_str);
        let actual = match uri {
            Some(uri) => {
                let actual = decode_data_uri(uri, "buffer")?.len() as u64;
                if actual != declared {
                    return invalid("data URI buffer length does not match byteLength");
                }
                actual
            }
            None if format == ModelFormat::Glb && index == 0 => {
                let bin = bin.ok_or_else(|| error("GLB buffer requires a BIN chunk"))?;
                let actual = bin.len() as u64;
                if actual < declared || actual > declared.saturating_add(3) {
                    return invalid(
                        "GLB BIN length exceeds declared buffer length or alignment padding",
                    );
                }
                if bin[declared as usize..].iter().any(|byte| *byte != 0) {
                    return invalid("GLB BIN alignment padding must be zero");
                }
                bin_consumed = true;
                actual
            }
            None => return invalid("buffer has no self-contained data"),
        };
        if actual < declared || actual > MAX_MODEL_BYTES {
            return invalid("buffer data does not satisfy declared byteLength");
        }
        buffer_lengths.push(declared);
    }
    if format == ModelFormat::Glb && bin.is_some() && !bin_consumed {
        return invalid("GLB has an unreferenced BIN chunk");
    }

    for view in &views {
        let buffer = index(view, "buffer", buffers.len())?;
        let offset = optional_uint(view, "byteOffset")?.unwrap_or(0);
        let length = uint(view, "byteLength")?;
        if length == 0 || !offset.is_multiple_of(4) {
            return invalid("bufferView length or 4-byte offset alignment is invalid");
        }
        checked_end(offset, length, buffer_lengths[buffer], "bufferView")?;
        if let Some(stride) = optional_uint(view, "byteStride")?
            && (!(4..=252).contains(&stride) || !stride.is_multiple_of(4))
        {
            return invalid("bufferView byteStride is invalid");
        }
    }

    let mut accessor_counts = Vec::with_capacity(accessors.len());
    for accessor in &accessors {
        let count = uint(accessor, "count")?;
        if count == 0 || count > MAX_ACCESSOR_COUNT {
            return invalid("accessor count must be between 1 and 10 million");
        }
        let component = component_size(uint(accessor, "componentType")?)?;
        let element_bytes =
            accessor_storage_size(accessor.get("type").and_then(Value::as_str), component)?;
        if let Some(view_index) = optional_index(accessor, "bufferView", views.len())? {
            let view = &views[view_index];
            let available = uint(view, "byteLength")?;
            let offset = optional_uint(accessor, "byteOffset")?.unwrap_or(0);
            let stride = optional_uint(view, "byteStride")?.unwrap_or(element_bytes);
            let view_offset = optional_uint(view, "byteOffset")?.unwrap_or(0);
            if !offset.is_multiple_of(component)
                || !view_offset.saturating_add(offset).is_multiple_of(component)
                || stride < element_bytes
                || !stride.is_multiple_of(component)
                || (view.get("byteStride").is_some() && !stride.is_multiple_of(4))
            {
                return invalid("accessor element exceeds bufferView stride");
            }
            let occupied = if count == 0 {
                0
            } else {
                stride
                    .checked_mul(count - 1)
                    .and_then(|v| v.checked_add(element_bytes))
                    .ok_or_else(|| error("accessor range overflow"))?
            };
            checked_end(offset, occupied, available, "accessor")?;
        } else if accessor.get("sparse").is_none() {
            return invalid("accessor without bufferView must be sparse");
        }
        validate_sparse(accessor, count, component, element_bytes, &views)?;
        accessor_counts.push(count);
    }

    for image in &images {
        match (
            image.get("uri").and_then(Value::as_str),
            image.get("bufferView"),
        ) {
            (Some(uri), None) => {
                decode_data_uri(uri, "image")?;
            }
            (None, Some(_)) => {
                index(image, "bufferView", views.len())?;
                if image.get("mimeType").and_then(Value::as_str).is_none() {
                    return invalid("bufferView image requires mimeType");
                }
            }
            _ => return invalid("image must use one self-contained data source"),
        }
    }
    for texture in &textures {
        index(texture, "source", images.len())?;
        if texture.get("sampler").is_some() {
            index(texture, "sampler", texture_samplers.len())?;
        }
    }
    for material in &materials {
        if let Some(pbr) = material.get("pbrMetallicRoughness") {
            validate_texture_info(pbr.get("baseColorTexture"), textures.len())?;
            validate_texture_info(pbr.get("metallicRoughnessTexture"), textures.len())?;
        }
        validate_texture_info(material.get("normalTexture"), textures.len())?;
        validate_texture_info(material.get("occlusionTexture"), textures.len())?;
        validate_texture_info(material.get("emissiveTexture"), textures.len())?;
    }
    let mut mesh_morph_counts = Vec::with_capacity(meshes.len());
    for mesh in &meshes {
        let primitives = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty() && values.len() <= MAX_RESOURCE_COUNT)
            .ok_or_else(|| error("mesh primitives are missing or excessive"))?;
        let mut morph_count = None;
        for primitive in primitives {
            let count = primitive
                .get("targets")
                .map(|targets| {
                    targets
                        .as_array()
                        .map(Vec::len)
                        .ok_or_else(|| error("morph targets must be an array"))
                })
                .transpose()?
                .unwrap_or(0);
            if morph_count
                .replace(count)
                .is_some_and(|previous| previous != count)
            {
                return invalid("mesh primitives have inconsistent morph target counts");
            }
        }
        mesh_morph_counts.push(morph_count.unwrap_or(0));
    }
    let mut parents = vec![None; nodes.len()];
    let mut children_by_node = vec![Vec::new(); nodes.len()];
    for (node_index, node) in nodes.iter().enumerate() {
        if node.get("mesh").is_some() {
            index(node, "mesh", meshes.len())?;
        }
        if node.get("skin").is_some() {
            index(node, "skin", skins.len())?;
        }
        if let Some(children) = node.get("children") {
            let children = children
                .as_array()
                .ok_or_else(|| error("node children must be an array"))?;
            for child in children {
                let child = value_index(child, nodes.len(), "node child")?;
                if child == node_index || parents[child].replace(node_index).is_some() {
                    return invalid("node graph has a self-reference or multiple parents");
                }
                children_by_node[node_index].push(child);
            }
        }
    }
    validate_node_graph(&children_by_node)?;
    for scene in &scenes {
        if let Some(scene_nodes) = scene.get("nodes") {
            let scene_nodes = scene_nodes
                .as_array()
                .ok_or_else(|| error("scene nodes must be an array"))?;
            for node in scene_nodes {
                value_index(node, nodes.len(), "scene node")?;
            }
        }
    }
    if let Some(default_scene) = root.get("scene") {
        value_index(default_scene, scenes.len(), "default scene")?;
    }

    for skin in &skins {
        let joints = skin
            .get("joints")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .ok_or_else(|| error("skin joints must be a non-empty array"))?;
        for joint in joints {
            value_index(joint, nodes.len(), "skin joint")?;
        }
        if skin.get("skeleton").is_some() {
            index(skin, "skeleton", nodes.len())?;
        }
        if let Some(accessor) = optional_index(skin, "inverseBindMatrices", accessors.len())?
            && (accessors[accessor].get("type").and_then(Value::as_str) != Some("MAT4")
                || uint(accessors[accessor], "componentType")? != 5126
                || accessor_counts[accessor] != joints.len() as u64)
        {
            return invalid("inverseBindMatrices must be one float MAT4 per joint");
        }
    }

    for animation in &animations {
        let samplers = animation
            .get("samplers")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty() && values.len() <= MAX_RESOURCE_COUNT)
            .ok_or_else(|| error("animation samplers are missing or excessive"))?;
        for sampler in samplers {
            let input = index(sampler, "input", accessors.len())?;
            index(sampler, "output", accessors.len())?;
            if accessors[input].get("type").and_then(Value::as_str) != Some("SCALAR")
                || uint(accessors[input], "componentType")? != 5126
            {
                return invalid("animation input must be a float scalar accessor");
            }
            let interpolation = sampler
                .get("interpolation")
                .and_then(Value::as_str)
                .unwrap_or("LINEAR");
            if !matches!(interpolation, "LINEAR" | "STEP" | "CUBICSPLINE") {
                return invalid("animation interpolation is invalid");
            }
        }
        let channels = animation
            .get("channels")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty() && values.len() <= MAX_RESOURCE_COUNT)
            .ok_or_else(|| error("animation channels are missing or excessive"))?;
        for channel in channels {
            let sampler_index = index(channel, "sampler", samplers.len())?;
            let target = channel
                .get("target")
                .ok_or_else(|| error("animation target is missing"))?;
            let target_node = index(target, "node", nodes.len())?;
            let path = target.get("path").and_then(Value::as_str);
            if !matches!(path, Some("translation" | "rotation" | "scale" | "weights")) {
                return invalid("animation target path is invalid");
            }
            let output = index(&samplers[sampler_index], "output", accessors.len())?;
            let input = index(&samplers[sampler_index], "input", accessors.len())?;
            let interpolation = samplers[sampler_index]
                .get("interpolation")
                .and_then(Value::as_str)
                .unwrap_or("LINEAR");
            let expected_type = match path.unwrap() {
                "translation" | "scale" => "VEC3",
                "rotation" => "VEC4",
                "weights" => "SCALAR",
                _ => unreachable!(),
            };
            if accessors[output].get("type").and_then(Value::as_str) != Some(expected_type) {
                return invalid("animation output accessor type does not match target path");
            }
            let multiplier = if interpolation == "CUBICSPLINE" { 3 } else { 1 };
            let morph_count = if path == Some("weights") {
                let mesh = nodes[target_node]
                    .get("mesh")
                    .ok_or_else(|| error("weight animation target node has no mesh"))?;
                let mesh = value_index(mesh, meshes.len(), "weight animation mesh")?;
                let count = mesh_morph_counts[mesh];
                if count == 0 {
                    return invalid("weight animation target mesh has no morph targets");
                }
                count as u64
            } else {
                1
            };
            let expected = accessor_counts[input]
                .checked_mul(multiplier)
                .and_then(|count| count.checked_mul(morph_count))
                .ok_or_else(|| error("animation output count overflow"))?;
            if accessor_counts[output] != expected {
                return invalid("animation output count does not exactly match its channel");
            }
        }
    }

    let mut vertex_count = 0u64;
    let mut triangle_count = 0u64;
    let mut primitive_count = 0u64;
    let mut bounds_min: Option<[f64; 3]> = None;
    let mut bounds_max: Option<[f64; 3]> = None;
    for mesh in &meshes {
        let primitives = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .ok_or_else(|| error("mesh primitives are required"))?;
        if primitives.is_empty() || primitives.len() > MAX_RESOURCE_COUNT {
            return invalid("mesh primitive count is invalid");
        }
        for primitive in primitives {
            primitive_count = primitive_count
                .checked_add(1)
                .ok_or_else(|| error("primitive count overflow"))?;
            if primitive_count > MAX_AGGREGATE_PRIMITIVES {
                return invalid("aggregate primitive count exceeds one million");
            }
            let position = primitive
                .get("attributes")
                .and_then(|v| v.get("POSITION"))
                .ok_or_else(|| error("mesh primitive has no POSITION"))?;
            let attributes = primitive
                .get("attributes")
                .and_then(Value::as_object)
                .ok_or_else(|| error("primitive attributes must be an object"))?;
            for accessor in attributes.values() {
                value_index(accessor, accessors.len(), "primitive attribute accessor")?;
            }
            if let Some(targets) = primitive.get("targets") {
                let targets = targets
                    .as_array()
                    .ok_or_else(|| error("morph targets must be an array"))?;
                if targets.len() > MAX_RESOURCE_COUNT {
                    return invalid("morph target count exceeds 4096");
                }
                for target in targets {
                    let target = target
                        .as_object()
                        .ok_or_else(|| error("morph target must be an object"))?;
                    for accessor in target.values() {
                        value_index(accessor, accessors.len(), "morph target accessor")?;
                    }
                }
            }
            let position_index = value_index(position, accessors.len(), "POSITION accessor")?;
            let accessor = &accessors[position_index];
            if accessor.get("type").and_then(Value::as_str) != Some("VEC3")
                || uint(accessor, "componentType")? != 5126
            {
                return invalid("POSITION must be a VEC3 float accessor");
            }
            let min = finite_vec3(accessor.get("min"), "POSITION min")?;
            let max = finite_vec3(accessor.get("max"), "POSITION max")?;
            if min.iter().zip(max).any(|(a, b)| *a > b) {
                return invalid("POSITION bounds are inverted");
            }
            merge_bounds(&mut bounds_min, &mut bounds_max, min, max);
            let vertices = accessor_counts[position_index];
            vertex_count = vertex_count
                .checked_add(vertices)
                .ok_or_else(|| error("vertex count overflow"))?;
            if vertex_count > MAX_AGGREGATE_VERTICES {
                return invalid("aggregate vertex count exceeds 10 million");
            }
            let elements = if let Some(indices) = primitive.get("indices") {
                let index = value_index(indices, accessors.len(), "index accessor")?;
                if accessors[index].get("type").and_then(Value::as_str) != Some("SCALAR")
                    || !matches!(uint(accessors[index], "componentType")?, 5121 | 5123 | 5125)
                {
                    return invalid("primitive indices must use an unsigned scalar accessor");
                }
                accessor_counts[index]
            } else {
                vertices
            };
            let mode = optional_uint(primitive, "mode")?.unwrap_or(4);
            let triangles = match mode {
                4 => elements / 3,
                5 | 6 => elements.saturating_sub(2),
                0..=3 => 0,
                _ => return invalid("primitive mode is invalid"),
            };
            triangle_count = triangle_count
                .checked_add(triangles)
                .ok_or_else(|| error("triangle count overflow"))?;
            if triangle_count > MAX_AGGREGATE_TRIANGLES {
                return invalid("aggregate triangle count exceeds 10 million");
            }
            if primitive.get("material").is_some() {
                index(primitive, "material", materials.len())?;
            }
        }
    }
    if primitive_count == 0 {
        return invalid("at least one mesh primitive is required");
    }
    Ok(Model3dMetadata {
        vertex_count,
        triangle_count,
        mesh_count: meshes.len() as u32,
        primitive_count: u32::try_from(primitive_count)
            .map_err(|_| error("primitive count cannot be represented"))?,
        material_count: materials.len() as u32,
        texture_count: textures.len() as u32,
        animation_count: animations.len() as u32,
        has_skin: !skins.is_empty(),
        bounds_min,
        bounds_max,
        gltf_version: "2.0".into(),
    })
}

fn validate_required_extensions(value: Option<&Value>) -> Result<(), ModelValidationError> {
    const STRUCTURAL: &[&str] = &[
        "KHR_materials_unlit",
        "KHR_texture_transform",
        "KHR_lights_punctual",
    ];
    if let Some(values) = value {
        for extension in values
            .as_array()
            .ok_or_else(|| error("extensionsRequired must be an array"))?
        {
            if !extension
                .as_str()
                .is_some_and(|name| STRUCTURAL.contains(&name))
            {
                return invalid("unsupported required extension");
            }
        }
    }
    Ok(())
}

fn validate_sparse(
    accessor: &Value,
    count: u64,
    component: u64,
    element_bytes: u64,
    views: &[&Value],
) -> Result<(), ModelValidationError> {
    let Some(sparse) = accessor.get("sparse") else {
        return Ok(());
    };
    let sparse_count = uint(sparse, "count")?;
    if sparse_count == 0 || sparse_count > count {
        return invalid("sparse accessor count is invalid");
    }
    let indices = sparse
        .get("indices")
        .ok_or_else(|| error("sparse indices are missing"))?;
    let indices_view = index(indices, "bufferView", views.len())?;
    let index_component = uint(indices, "componentType")?;
    if !matches!(index_component, 5121 | 5123 | 5125) {
        return invalid("sparse index component type is invalid");
    }
    let index_bytes = component_size(index_component)?
        .checked_mul(sparse_count)
        .ok_or_else(|| error("sparse index range overflow"))?;
    let indices_offset = optional_uint(indices, "byteOffset")?.unwrap_or(0);
    if !indices_offset.is_multiple_of(component_size(index_component)?) {
        return invalid("sparse index offset is misaligned");
    }
    checked_end(
        indices_offset,
        index_bytes,
        uint(views[indices_view], "byteLength")?,
        "sparse indices",
    )?;
    let values = sparse
        .get("values")
        .ok_or_else(|| error("sparse values are missing"))?;
    let values_view = index(values, "bufferView", views.len())?;
    let value_bytes = element_bytes
        .checked_mul(sparse_count)
        .ok_or_else(|| error("sparse value range overflow"))?;
    let values_offset = optional_uint(values, "byteOffset")?.unwrap_or(0);
    if !values_offset.is_multiple_of(component) {
        return invalid("sparse value offset is misaligned");
    }
    checked_end(
        values_offset,
        value_bytes,
        uint(views[values_view], "byteLength")?,
        "sparse values",
    )?;
    Ok(())
}

fn decode_data_uri(uri: &str, kind: &str) -> Result<Vec<u8>, ModelValidationError> {
    if uri.contains(['\\', '?', '#']) || uri.to_ascii_lowercase().contains("%2e") {
        return invalid("resource URI contains an unsafe path or suffix");
    }
    let Some((header, encoded)) = uri.split_once(',') else {
        return invalid("external resource URI is forbidden");
    };
    if !header.starts_with("data:")
        || !header.ends_with(";base64")
        || encoded.len() > MAX_DATA_URI_BYTES.saturating_mul(4) / 3 + 4
    {
        return invalid("only bounded base64 data URIs are allowed");
    }
    let bytes = base64::prelude::BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| error("data URI base64 is invalid"))?;
    if bytes.len() > MAX_DATA_URI_BYTES {
        return invalid(&format!("{kind} data URI exceeds 64 MiB"));
    }
    Ok(bytes)
}

fn array<'a>(root: &'a Value, key: &str) -> Result<Vec<&'a Value>, ModelValidationError> {
    Ok(match root.get(key) {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .ok_or_else(|| error(&format!("{key} must be an array")))?
            .iter()
            .collect(),
    })
}
fn uint(value: &Value, key: &str) -> Result<u64, ModelValidationError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| error(&format!("{key} must be an unsigned integer")))
}
fn optional_uint(value: &Value, key: &str) -> Result<Option<u64>, ModelValidationError> {
    value.get(key).map(|_| uint(value, key)).transpose()
}
fn index(value: &Value, key: &str, len: usize) -> Result<usize, ModelValidationError> {
    value_index(
        value
            .get(key)
            .ok_or_else(|| error(&format!("{key} is required")))?,
        len,
        key,
    )
}
fn optional_index(
    value: &Value,
    key: &str,
    len: usize,
) -> Result<Option<usize>, ModelValidationError> {
    value.get(key).map(|v| value_index(v, len, key)).transpose()
}
fn value_index(value: &Value, len: usize, label: &str) -> Result<usize, ModelValidationError> {
    let value = value
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v < len)
        .ok_or_else(|| error(&format!("{label} reference is out of range")))?;
    Ok(value)
}
fn checked_end(
    offset: u64,
    length: u64,
    available: u64,
    label: &str,
) -> Result<(), ModelValidationError> {
    if offset.checked_add(length).is_none_or(|end| end > available) {
        invalid(&format!("{label} range exceeds its parent buffer"))
    } else {
        Ok(())
    }
}
fn component_size(value: u64) -> Result<u64, ModelValidationError> {
    match value {
        5120 | 5121 => Ok(1),
        5122 | 5123 => Ok(2),
        5125 | 5126 => Ok(4),
        _ => invalid("accessor componentType is invalid"),
    }
}
fn accessor_storage_size(value: Option<&str>, component: u64) -> Result<u64, ModelValidationError> {
    let (columns, rows) = match value {
        Some("SCALAR") => return Ok(component),
        Some("VEC2") => {
            return component
                .checked_mul(2)
                .ok_or_else(|| error("accessor size overflow"));
        }
        Some("VEC3") => {
            return component
                .checked_mul(3)
                .ok_or_else(|| error("accessor size overflow"));
        }
        Some("VEC4") => {
            return component
                .checked_mul(4)
                .ok_or_else(|| error("accessor size overflow"));
        }
        Some("MAT2") => (2, 2),
        Some("MAT3") => (3, 3),
        Some("MAT4") => (4, 4),
        _ => return invalid("accessor type is invalid"),
    };
    let column = component
        .checked_mul(rows)
        .ok_or_else(|| error("matrix accessor size overflow"))?;
    let aligned_column = column
        .checked_add(3)
        .map(|value| value / 4 * 4)
        .ok_or_else(|| error("matrix accessor size overflow"))?;
    aligned_column
        .checked_mul(columns)
        .ok_or_else(|| error("matrix accessor size overflow"))
}
fn validate_texture_info(
    value: Option<&Value>,
    texture_count: usize,
) -> Result<(), ModelValidationError> {
    let Some(value) = value else {
        return Ok(());
    };
    index(value, "index", texture_count)?;
    if value.get("texCoord").is_some() {
        uint(value, "texCoord")?;
    }
    Ok(())
}
fn validate_node_graph(children: &[Vec<usize>]) -> Result<(), ModelValidationError> {
    let mut colors = vec![0u8; children.len()];
    for start in 0..children.len() {
        if colors[start] != 0 {
            continue;
        }
        let mut stack = vec![(start, false)];
        while let Some((node, exiting)) = stack.pop() {
            if exiting {
                colors[node] = 2;
                continue;
            }
            if colors[node] == 1 {
                return invalid("node graph contains a cycle");
            }
            if colors[node] == 2 {
                continue;
            }
            colors[node] = 1;
            stack.push((node, true));
            for &child in children[node].iter().rev() {
                if colors[child] == 1 {
                    return invalid("node graph contains a cycle");
                }
                stack.push((child, false));
            }
        }
    }
    Ok(())
}
fn finite_vec3(value: Option<&Value>, label: &str) -> Result<[f64; 3], ModelValidationError> {
    let values = value
        .and_then(Value::as_array)
        .filter(|v| v.len() == 3)
        .ok_or_else(|| error(&format!("{label} is required")))?;
    let mut out = [0.0; 3];
    for (i, value) in values.iter().enumerate() {
        out[i] = value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| error(&format!("{label} must be finite")))?;
    }
    Ok(out)
}
fn merge_bounds(
    minimum: &mut Option<[f64; 3]>,
    maximum: &mut Option<[f64; 3]>,
    min: [f64; 3],
    max: [f64; 3],
) {
    let minimum = minimum.get_or_insert(min);
    let maximum = maximum.get_or_insert(max);
    for i in 0..3 {
        minimum[i] = minimum[i].min(min[i]);
        maximum[i] = maximum[i].max(max[i]);
    }
}
fn error(message: &str) -> ModelValidationError {
    ModelValidationError::Invalid(message.into())
}
fn invalid<T>(message: &str) -> Result<T, ModelValidationError> {
    Err(error(message))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn fixture_json(buffer_uri: Option<&str>) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "asset":{"version":"2.0"},
            "buffers":[{"byteLength":36,"uri":buffer_uri}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"mode":4}]}]
        })).unwrap()
    }

    pub(crate) fn fixture_glb() -> Vec<u8> {
        let mut json = fixture_json(None);
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let bin = vec![0u8; 36];
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&JSON_CHUNK.to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&BIN_CHUNK.to_le_bytes());
        out.extend_from_slice(&bin);
        out
    }

    fn self_contained_value() -> Value {
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::prelude::BASE64_STANDARD.encode([0u8; 36])
        );
        serde_json::from_slice(&fixture_json(Some(&uri))).unwrap()
    }

    fn rejects(value: &Value) {
        assert!(validate(&serde_json::to_vec(value).unwrap(), ModelFormat::Gltf).is_err());
    }

    #[test]
    fn validates_glb_and_self_contained_gltf_metadata() {
        let glb = validate(&fixture_glb(), ModelFormat::Glb).unwrap();
        assert_eq!(
            (glb.metadata.vertex_count, glb.metadata.triangle_count),
            (3, 1)
        );
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::prelude::BASE64_STANDARD.encode([0u8; 36])
        );
        let gltf = validate(&fixture_json(Some(&uri)), ModelFormat::Gltf).unwrap();
        assert_eq!(gltf.validation_level, "structural-self-contained");
    }

    #[test]
    fn rejects_malformed_glb_and_external_resources() {
        for bytes in [
            vec![],
            b"bad!".to_vec(),
            {
                let mut v = fixture_glb();
                v[4] = 1;
                v
            },
            {
                let mut v = fixture_glb();
                v.pop();
                v
            },
        ] {
            assert!(validate(&bytes, ModelFormat::Glb).is_err());
        }
        for uri in [
            "scene.bin",
            "../scene.bin",
            "https://example/x",
            "file:///x",
            "C:\\x",
            "data:application/octet-stream;base64,AA==?x",
        ] {
            assert!(
                validate(&fixture_json(Some(uri)), ModelFormat::Gltf).is_err(),
                "{uri}"
            );
        }

        let mut nul_padding = fixture_glb();
        let json_length = u32::from_le_bytes(nul_padding[12..16].try_into().unwrap()) as usize;
        nul_padding[20 + json_length - 1] = 0;
        assert!(validate(&nul_padding, ModelFormat::Glb).is_err());

        let mut excessive_bin = fixture_glb();
        let bin_header = 20 + json_length;
        let bin_length = u32::from_le_bytes(
            excessive_bin[bin_header..bin_header + 4]
                .try_into()
                .unwrap(),
        );
        excessive_bin[bin_header..bin_header + 4].copy_from_slice(&(bin_length + 4).to_le_bytes());
        excessive_bin.extend_from_slice(&[0; 4]);
        let total = excessive_bin.len() as u32;
        excessive_bin[8..12].copy_from_slice(&total.to_le_bytes());
        assert!(validate(&excessive_bin, ModelFormat::Glb).is_err());

        // Unknown chunks are intentionally outside the controlled foundation profile.
        let mut unknown_chunk = fixture_glb();
        unknown_chunk[bin_header + 4..bin_header + 8]
            .copy_from_slice(&0x1234_5678u32.to_le_bytes());
        assert!(validate(&unknown_chunk, ModelFormat::Glb).is_err());
    }

    #[test]
    fn rejects_bad_accessor_layouts_and_all_dangling_primitive_references() {
        let mut value = self_contained_value();
        value["accessors"][0]["count"] = serde_json::json!(0);
        rejects(&value);

        let mut value = self_contained_value();
        value["bufferViews"][0]["byteOffset"] = serde_json::json!(2);
        rejects(&value);

        let mut value = self_contained_value();
        value["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView":0,"componentType":5123,"count":1,"type":"MAT3","byteOffset":16}));
        value["meshes"][0]["primitives"][0]["attributes"]["CUSTOM"] = serde_json::json!(1);
        rejects(&value);

        let mut value = self_contained_value();
        value["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = serde_json::json!(99);
        rejects(&value);

        let mut value = self_contained_value();
        value["meshes"][0]["primitives"][0]["targets"] = serde_json::json!([{"POSITION":99}]);
        rejects(&value);

        let mut value = self_contained_value();
        value["accessors"][0]
            .as_object_mut()
            .unwrap()
            .remove("bufferView");
        value["accessors"][0]["count"] = serde_json::json!(10_000_001u64);
        value["accessors"][0]["sparse"] = serde_json::json!({
            "count":1,
            "indices":{"bufferView":0,"componentType":5121,"byteOffset":0},
            "values":{"bufferView":0,"byteOffset":0}
        });
        rejects(&value);

        let mut value = self_contained_value();
        value["accessors"][0]
            .as_object_mut()
            .unwrap()
            .remove("bufferView");
        value["accessors"][0]["count"] = serde_json::json!(10_000_000u64);
        value["accessors"][0]["sparse"] = serde_json::json!({
            "count":1,
            "indices":{"bufferView":0,"componentType":5121,"byteOffset":0},
            "values":{"bufferView":0,"byteOffset":0}
        });
        value["meshes"][0]["primitives"] = serde_json::json!([
            {"attributes":{"POSITION":0},"mode":0},
            {"attributes":{"POSITION":0},"mode":0}
        ]);
        rejects(&value);
    }

    #[test]
    fn rejects_invalid_node_scene_skin_animation_and_material_graphs() {
        let mut value = self_contained_value();
        value["nodes"] = serde_json::json!([{"children":[1]},{"children":[0]}]);
        rejects(&value);

        let mut value = self_contained_value();
        value["nodes"] = serde_json::json!([{"children":[2]},{"children":[2]},{}]);
        rejects(&value);

        let mut value = self_contained_value();
        value["nodes"] = serde_json::json!([{}]);
        value["scenes"] = serde_json::json!([{"nodes":[2]}]);
        value["scene"] = serde_json::json!(0);
        rejects(&value);

        let mut value = self_contained_value();
        value["nodes"] = serde_json::json!([{}]);
        value["skins"] = serde_json::json!([{"joints":[0],"inverseBindMatrices":0}]);
        rejects(&value);

        let mut value = self_contained_value();
        value["nodes"] = serde_json::json!([{}]);
        value["animations"] = serde_json::json!([{"samplers":[{"input":0,"output":0,"interpolation":"BEZIER"}],"channels":[{"sampler":0,"target":{"node":0,"path":"translation"}}]}]);
        rejects(&value);

        let mut value = self_contained_value();
        value["materials"] =
            serde_json::json!([{"pbrMetallicRoughness":{"baseColorTexture":{"index":1}}}]);
        value["meshes"][0]["primitives"][0]["material"] = serde_json::json!(0);
        rejects(&value);
    }

    fn animation_value(path: &str, interpolation: &str, output_count: u64) -> Value {
        let mut value = self_contained_value();
        let uri = format!(
            "data:application/octet-stream;base64,{}",
            base64::prelude::BASE64_STANDARD.encode([0u8; 128])
        );
        value["buffers"][0] = serde_json::json!({"byteLength":128,"uri":uri});
        value["bufferViews"][0]["byteLength"] = serde_json::json!(128);
        let output_type = match path {
            "weights" => "SCALAR",
            "rotation" => "VEC4",
            _ => "VEC3",
        };
        value["accessors"].as_array_mut().unwrap().extend([
            serde_json::json!({"bufferView":0,"byteOffset":36,"componentType":5126,"count":2,"type":"SCALAR"}),
            serde_json::json!({"bufferView":0,"byteOffset":44,"componentType":5126,"count":output_count,"type":output_type}),
        ]);
        if path == "weights" {
            value["meshes"][0]["primitives"][0]["targets"] = serde_json::json!([{"POSITION":0}]);
        }
        value["nodes"] = serde_json::json!([{"mesh":0}]);
        value["animations"] = serde_json::json!([{
            "samplers":[{"input":1,"output":2,"interpolation":interpolation}],
            "channels":[{"sampler":0,"target":{"node":0,"path":path}}]
        }]);
        value
    }

    #[test]
    fn animation_output_counts_are_exact_for_transforms_and_weights() {
        for value in [
            animation_value("translation", "LINEAR", 2),
            animation_value("translation", "CUBICSPLINE", 6),
            animation_value("weights", "LINEAR", 2),
            animation_value("weights", "CUBICSPLINE", 6),
        ] {
            validate(&serde_json::to_vec(&value).unwrap(), ModelFormat::Gltf).unwrap();
        }
        rejects(&animation_value("translation", "LINEAR", 1));
        rejects(&animation_value("translation", "CUBICSPLINE", 5));
        rejects(&animation_value("weights", "LINEAR", 1));
        let mut missing_targets = animation_value("weights", "LINEAR", 2);
        missing_targets["meshes"][0]["primitives"][0]
            .as_object_mut()
            .unwrap()
            .remove("targets");
        rejects(&missing_targets);
    }
}
