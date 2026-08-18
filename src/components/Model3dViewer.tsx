import { useEffect, useRef, useState } from "react";
import * as THREE from "three";
import { GLTFLoader, OrbitControls } from "three-stdlib";

type Model3dViewerProps = {
  assetId: string;
  presetView?: "front" | "rear" | "left" | "right" | "perspective" | "reset";
  onLoad?: () => void;
  onError?: (error: string) => void;
};

const PRESET_CAMERA_POSITIONS: Record<string, [number, number, number]> = {
  front: [0, 5, 10],
  rear: [0, 5, -10],
  left: [-10, 5, 0],
  right: [10, 5, 0],
  perspective: [7, 7, 7],
  reset: [0, 0, 0], // Will be computed to fit model
};

function isMesh(obj: THREE.Object3D): obj is THREE.Mesh {
  return (obj as any).isMesh === true;
}

function isStandardMaterial(mat: THREE.Material): mat is THREE.MeshStandardMaterial {
  return mat.type === "MeshStandardMaterial";
}

export function Model3dViewer({ assetId, presetView = "perspective", onLoad, onError }: Model3dViewerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [state, setState] = useState<"loading" | "ready" | "error">("loading");
  const [modelInfo, setModelInfo] = useState<{
    materials: number;
    textures: number;
    triangles: number;
    hasTextures: boolean;
  } | null>(null);
  const [currentPreset, setCurrentPreset] = useState(presetView);

  useEffect(() => {
    if (!canvasRef.current || !assetId) return;

    let mounted = true;
    let animationFrameId: number | null = null;
    let renderer: THREE.WebGLRenderer | null = null;
    let scene: THREE.Scene | null = null;
    let camera: THREE.PerspectiveCamera | null = null;
    let controls: OrbitControls | null = null;

    const loadModel = async () => {
      try {
        const canvas = canvasRef.current!;
        const width = canvas.clientWidth;
        const height = canvas.clientHeight;

        // Renderer
        renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
        renderer.setSize(width, height);
        renderer.setPixelRatio(window.devicePixelRatio);
        renderer.outputColorSpace = THREE.SRGBColorSpace;
        renderer.shadowMap.enabled = true;
        renderer.shadowMap.type = THREE.PCFSoftShadowMap;
        renderer.toneMapping = THREE.ACESFilmicToneMapping;
        renderer.toneMappingExposure = 1.2;

        // Scene
        scene = new THREE.Scene();
        scene.background = new THREE.Color(0x1a1a2e);

        // Camera
        camera = new THREE.PerspectiveCamera(45, width / height, 0.1, 1000);
        camera.position.set(7, 7, 7);

        // Controls
        controls = new OrbitControls(camera, canvas);
        controls.enableDamping = true;
        controls.dampingFactor = 0.05;
        controls.enablePan = true;
        controls.minDistance = 1;
        controls.maxDistance = 100;
        controls.target.set(0, 0, 0);

        // Lights
        const ambientLight = new THREE.AmbientLight(0xffffff, 0.5);
        scene.add(ambientLight);

        const dirLight1 = new THREE.DirectionalLight(0xffffff, 1.5);
        dirLight1.position.set(5, 10, 7);
        dirLight1.castShadow = true;
        dirLight1.shadow.mapSize.width = 2048;
        dirLight1.shadow.mapSize.height = 2048;
        scene.add(dirLight1);

        const dirLight2 = new THREE.DirectionalLight(0xffffff, 0.5);
        dirLight2.position.set(-5, 5, -7);
        scene.add(dirLight2);

        const hemiLight = new THREE.HemisphereLight(0xffffff, 0x444444, 0.3);
        scene.add(hemiLight);

        // Grid helper
        const grid = new THREE.GridHelper(20, 20, 0x444466, 0x222244);
        scene.add(grid);

        // Load GLB
        const loader = new GLTFLoader();
        const url = `nexora-media://${assetId}`;
        
        const gltf = await loader.loadAsync(url);
        
        if (!mounted) return;

        const model = gltf.scene;
        scene.add(model);

        // Compute model bounds and center
        const box = new THREE.Box3().setFromObject(model);
        const center = new THREE.Vector3();
        box.getCenter(center);
        const size = new THREE.Vector3();
        box.getSize(size);

        // Center model
        model.position.sub(center);
        controls.target.set(0, 0, 0);

        // Fit camera to model
        const maxDim = Math.max(size.x, size.y, size.z);
        const fitDistance = maxDim * 1.5;
        camera.position.set(fitDistance, fitDistance * 0.8, fitDistance);
        camera.near = Math.max(0.1, maxDim * 0.01);
        camera.far = Math.max(1000, fitDistance * 10);
        camera.updateProjectionMatrix();

        // Apply preset view if specified
        if (currentPreset !== "reset" && currentPreset !== "perspective") {
          const pos = PRESET_CAMERA_POSITIONS[currentPreset];
          if (pos) {
            const distance = fitDistance;
            camera.position.set(pos[0] * (distance / 10), pos[1] * (distance / 10), pos[2] * (distance / 10));
            camera.lookAt(0, 0, 0);
          }
        }

        // Collect model info
        let materialCount = 0;
        let textureCount = 0;
        let triangleCount = 0;
        let hasTextures = false;

        model.traverse((child) => {
          if (isMesh(child)) {
            const mesh = child;
            const geometry = mesh.geometry;
            if (geometry.index) {
              triangleCount += geometry.index.count / 3;
            } else if (geometry.attributes.position) {
              triangleCount += geometry.attributes.position.count / 3;
            }

            const materials = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
            for (const mat of materials) {
              materialCount++;
              if (isStandardMaterial(mat)) {
                if (mat.map) { textureCount++; hasTextures = true; }
                if (mat.normalMap) { textureCount++; hasTextures = true; }
                if (mat.roughnessMap) { textureCount++; }
                if (mat.metalnessMap) { textureCount++; }
                if (mat.emissiveMap) { textureCount++; }
                if (mat.aoMap) { textureCount++; }
                if (mat.alphaMap) { textureCount++; }
              }
            }
          }
        });

        setModelInfo({ materials: materialCount, textures: textureCount, triangles: triangleCount, hasTextures });

        if (mounted) {
          setState("ready");
          onLoad?.();
        }

        // Animation loop
        const animate = () => {
          if (!mounted) return;
          animationFrameId = requestAnimationFrame(animate);
          controls?.update();
          renderer?.render(scene!, camera!);
        };
        animate();

      } catch (error) {
        console.error("Failed to load model:", error);
        if (mounted) {
          setState("error");
          onError?.(error instanceof Error ? error.message : "Failed to load model");
        }
      }
    };

    loadModel();

    // Handle resize
    const handleResize = () => {
      if (!renderer || !camera || !canvasRef.current) return;
      const width = canvasRef.current.clientWidth;
      const height = canvasRef.current.clientHeight;
      camera.aspect = width / height;
      camera.updateProjectionMatrix();
      renderer.setSize(width, height);
    };

    window.addEventListener("resize", handleResize);

    return () => {
      mounted = false;
      if (animationFrameId) cancelAnimationFrame(animationFrameId);
      controls?.dispose();
      renderer?.dispose();
      scene?.traverse((obj) => {
        if (isMesh(obj)) {
          obj.geometry.dispose();
          const mats = Array.isArray(obj.material) ? obj.material : [obj.material];
          for (const mat of mats) {
            Object.values(mat).forEach((value) => {
              if (value && typeof value === "object" && "dispose" in value) {
                (value as any).dispose?.();
              }
            });
          }
        }
      });
      window.removeEventListener("resize", handleResize);
    };
  }, [assetId, presetView, currentPreset, onLoad, onError]);

  // Update currentPreset when presetView prop changes
  useEffect(() => {
    setCurrentPreset(presetView);
  }, [presetView]);

  if (state === "loading") {
    return (
      <div className="model3d-viewer model3d-viewer--loading">
        <canvas ref={canvasRef} className="model3d-viewer__canvas" />
        <div className="model3d-viewer__overlay">
          <div className="model3d-viewer__spinner" />
          <p>Loading model...</p>
        </div>
      </div>
    );
  }

  if (state === "error") {
    return (
      <div className="model3d-viewer model3d-viewer--error">
        <canvas ref={canvasRef} className="model3d-viewer__canvas" />
        <div className="model3d-viewer__overlay">
          <p className="model3d-viewer__error">Failed to load model</p>
          <button className="btn btn--secondary" onClick={() => setState("loading")}>Retry</button>
        </div>
      </div>
    );
  }

  const handlePresetChange = (preset: "front" | "rear" | "left" | "right" | "perspective" | "reset") => {
    setCurrentPreset(preset);
  };

  return (
    <div className="model3d-viewer model3d-viewer--ready">
      <canvas ref={canvasRef} className="model3d-viewer__canvas" />
      <div className="model3d-viewer__controls">
        <div className="model3d-viewer__presets" role="group" aria-label="Camera presets">
          <button
            className={`model3d-viewer__preset-btn ${currentPreset === "front" ? "active" : ""}`}
            onClick={() => handlePresetChange("front")}
            disabled={currentPreset === "front"}
            title="Front view"
          >
            FRONT
          </button>
          <button
            className={`model3d-viewer__preset-btn ${currentPreset === "rear" ? "active" : ""}`}
            onClick={() => handlePresetChange("rear")}
            disabled={currentPreset === "rear"}
            title="Rear view"
          >
            REAR
          </button>
          <button
            className={`model3d-viewer__preset-btn ${currentPreset === "left" ? "active" : ""}`}
            onClick={() => handlePresetChange("left")}
            disabled={currentPreset === "left"}
            title="Left view"
          >
            LEFT
          </button>
          <button
            className={`model3d-viewer__preset-btn ${currentPreset === "right" ? "active" : ""}`}
            onClick={() => handlePresetChange("right")}
            disabled={currentPreset === "right"}
            title="Right view"
          >
            RIGHT
          </button>
          <button
            className={`model3d-viewer__preset-btn ${currentPreset === "perspective" ? "active" : ""}`}
            onClick={() => handlePresetChange("perspective")}
            disabled={currentPreset === "perspective"}
            title="Perspective view"
          >
            PERSPECTIVE
          </button>
          <button
            className={`model3d-viewer__preset-btn ${currentPreset === "reset" ? "active" : ""}`}
            onClick={() => handlePresetChange("reset")}
            disabled={currentPreset === "reset"}
            title="Reset to fit"
          >
            RESET
          </button>
        </div>
        {modelInfo && (
          <div className="model3d-viewer__info">
            <span>{modelInfo.triangles.toLocaleString()} triangles</span>
            <span>{modelInfo.materials} materials</span>
            <span>{modelInfo.textures} textures</span>
            {modelInfo.hasTextures && <span className="has-textures">✓ Textures</span>}
          </div>
        )}
      </div>
    </div>
  );
}

export default Model3dViewer;