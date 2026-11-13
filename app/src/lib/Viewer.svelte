<script lang="ts">
  // A model's .glb (from model_preview) in 3D: drag to turn, wheel to zoom, right-drag to move.
  import { onMount } from "svelte";
  import * as THREE from "three";
  import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
  import { OrbitControls } from "three/addons/controls/OrbitControls.js";

  let {
    glb,
    loading = false,
    error = null,
    empty = "Pick a model to see it here",
  }: { glb: ArrayBuffer | null; loading?: boolean; error?: string | null; empty?: string } = $props();

  let host: HTMLDivElement;
  let renderer: THREE.WebGLRenderer | null = null;
  let camera: THREE.PerspectiveCamera;
  let controls: OrbitControls;
  const scene = new THREE.Scene();
  let model: THREE.Object3D | null = null;
  let grid: THREE.GridHelper | null = null;
  let home = { position: new THREE.Vector3(), target: new THREE.Vector3() };
  let parseError = $state<string | null>(null);
  let shown = $state(false);

  function render() {
    renderer?.render(scene, camera);
  }

  function dispose(object: THREE.Object3D) {
    object.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (mesh.geometry) mesh.geometry.dispose();
      const materials = mesh.material ? (Array.isArray(mesh.material) ? mesh.material : [mesh.material]) : [];
      for (const m of materials) {
        for (const value of Object.values(m)) {
          if (value instanceof THREE.Texture) value.dispose();
        }
        m.dispose();
      }
    });
  }

  function clear() {
    if (model) {
      scene.remove(model);
      dispose(model);
      model = null;
    }
    if (grid) {
      scene.remove(grid);
      grid.dispose();
      grid = null;
    }
    shown = false;
  }

  /** Frames the model from the front, a little above. */
  function frame(object: THREE.Object3D) {
    const box = new THREE.Box3().setFromObject(object);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    const radius = Math.max(size.length() / 2, 0.01);
    const distance = radius / Math.sin(THREE.MathUtils.degToRad(camera.fov / 2)) * 0.9;
    camera.near = distance / 100;
    camera.far = distance * 100;
    camera.updateProjectionMatrix();
    home.target.copy(center);
    home.position.set(center.x, center.y + radius * 0.15, center.z + distance);
    reset();
    const step = Math.pow(10, Math.floor(Math.log10(radius)));
    grid = new THREE.GridHelper(step * 40, 40, 0x888888, 0x888888);
    (grid.material as THREE.Material).opacity = 0.18;
    (grid.material as THREE.Material).transparent = true;
    grid.position.set(center.x, box.min.y, center.z);
    scene.add(grid);
  }

  export function reset() {
    camera.position.copy(home.position);
    controls.target.copy(home.target);
    controls.update();
    render();
  }

  async function show(data: ArrayBuffer) {
    parseError = null;
    try {
      const gltf = await new GLTFLoader().parseAsync(data, "");
      clear();
      model = gltf.scene;
      scene.add(model);
      frame(model);
      shown = true;
      render();
    } catch (e) {
      clear();
      parseError = e instanceof Error ? e.message : String(e);
    }
  }

  $effect(() => {
    // (glb is read first so the effect runs again when it changes, also before the renderer exists)
    const data = glb;
    if (!renderer) return;
    if (data) void show(data);
    else {
      clear();
      render();
    }
  });

  onMount(() => {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    host.appendChild(renderer.domElement);
    camera = new THREE.PerspectiveCamera(35, 1, 0.01, 1000);
    controls = new OrbitControls(camera, renderer.domElement);
    controls.addEventListener("change", render);
    scene.add(new THREE.HemisphereLight(0xffffff, 0x50555c, 2.2));
    const key = new THREE.DirectionalLight(0xffffff, 1.6);
    key.position.set(1, 2, 3);
    scene.add(key);
    const back = new THREE.DirectionalLight(0xffffff, 0.6);
    back.position.set(-2, 1, -3);
    scene.add(back);

    const resize = () => {
      const w = Math.max(host.clientWidth, 1);
      const h = Math.max(host.clientHeight, 1);
      renderer!.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      render();
    };
    const observer = new ResizeObserver(resize);
    observer.observe(host);
    resize();
    if (glb) void show(glb);
    return () => {
      observer.disconnect();
      clear();
      controls.dispose();
      renderer?.dispose();
      renderer = null;
    };
  });
</script>

<div class="viewer">
  <div class="canvas" bind:this={host}></div>
  {#if loading}
    <div class="overlay"><span class="spinner"></span> Loading the model…</div>
  {:else if error || parseError}
    <div class="overlay bad">{error ?? parseError}</div>
  {:else if !glb}
    <div class="overlay muted">{empty}</div>
  {/if}
  {#if shown && !loading}
    <button class="reset ghost small" onclick={reset} title="Back to the front view">Front view</button>
    <div class="hint">Drag to turn · wheel to zoom · right-drag to move</div>
  {/if}
</div>

<style>
  .viewer {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 240px;
    background: var(--viewer-bg);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .canvas,
  .canvas :global(canvas) {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 24px;
    text-align: center;
    color: var(--text);
    pointer-events: none;
  }
  .overlay.muted {
    color: var(--muted);
  }
  .overlay.bad {
    color: var(--bad);
  }
  .reset {
    position: absolute;
    top: 10px;
    right: 10px;
  }
  .hint {
    position: absolute;
    left: 12px;
    bottom: 10px;
    font-size: 12px;
    color: var(--muted);
    pointer-events: none;
  }
</style>
