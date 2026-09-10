// Mesh — the structure of the fleet: nodes → repos → agents, plus the
// channels/links between them, editable in place. Two views of the same
// data: the map (drawn) and the list (dense, for big fleets).

import { useSearchParams } from "react-router-dom";
import MeshMap from "./MeshMap";
import MeshList from "./Library";

export default function Mesh() {
  const [params, setParams] = useSearchParams();
  // The list is where the work happens; the map is the picture. List first.
  const view = params.get("view") === "map" ? "map" : "list";
  const toggle = (
    <div className="class-select" role="tablist" aria-label="mesh view">
      {(["list", "map"] as const).map((v) => (
        <button
          key={v}
          type="button"
          role="tab"
          aria-selected={view === v}
          onClick={() => setParams(v === "list" ? {} : { view: v })}
          style={{
            background: view === v ? "var(--bg-strip-2)" : "var(--bg-well)",
            color: view === v ? "var(--text-hi)" : "var(--text-dim)",
          }}
        >
          {v}
        </button>
      ))}
    </div>
  );
  return (
    <>
      <div className="phone-note">The mesh views are dense: they work here, but they are laid out for a wider screen.</div>
      {view === "map" ? <MeshMap toggle={toggle} /> : <MeshList toggle={toggle} />}
    </>
  );
}
