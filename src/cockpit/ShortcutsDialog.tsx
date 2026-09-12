import { KEYBOARD_SHORTCUTS } from "../application/shortcuts.ts";
import { Dialog } from "./Dialog.tsx";

export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  const groups = ["Panels", "Sessions", "Conversation", "Help"] as const;
  return (
    <Dialog title="Keyboard shortcuts" onClose={onClose}>
      <p>Windows-first. Ctrl shortcuts also accept the Command key.</p>
      {groups.map((group) => (
        <section key={group} className="shortcut-group">
          <h3>{group}</h3>
          <table>
            <thead>
              <tr>
                <th scope="col">Shortcut</th>
                <th scope="col">Action</th>
              </tr>
            </thead>
            <tbody>
              {KEYBOARD_SHORTCUTS.filter((item) => item.group === group).map((item) => (
                <tr key={item.id}>
                  <th scope="row">
                    <kbd>{item.keys}</kbd>
                  </th>
                  <td>{item.label}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ))}
    </Dialog>
  );
}
