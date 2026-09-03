// Mesh — the structure of the fleet: nodes → repos → agents, plus the
// channels/links between them, editable in place. Two views of the same
// data: the map (drawn) and the list (dense, for big fleets).

import { useSearchParams } from "react-router-dom";
import MeshMap from "./MeshMap";
import MeshList from "./Library";

export default function Mesh() {
  const [params, setParams] = useSearchParams();
  const view = params.get("view") === "list" ? "list" : "map";
  const toggle = (
    <div className="class-select" role="tablist" aria-label="mesh view">
      {(["map", "list"] as const).map((v) => (
        <button
          key={v}
          type="button"
          role="tab"
          aria-selected={view === v}
          onClick={() => setParams(v === "map" ? {} : { view: v })}
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
  return view === "map" ? <MeshMap toggle={toggle} /> : <MeshList toggle={toggle} />;
}
