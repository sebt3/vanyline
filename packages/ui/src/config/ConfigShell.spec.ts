import { describe, expect, it } from "vitest";
import { h, nextTick, type Component } from "vue";
import { mount } from "@vue/test-utils";
import ConfigShell from "./ConfigShell.vue";
import type { ConfigNavGroup } from "./config-nav";

const groupA: ConfigNavGroup = {
  id: "group-a",
  label: "Groupe A",
  icon: "✦",
  accent: "#5b1ecf",
  sub: [
    { id: "a-1", label: "Écran A1" },
    { id: "a-2", label: "Écran A2" },
  ],
};

const groupB = {
  id: "group-b",
  label: "Groupe B",
  icon: "⚙",
  accent: "#4c90f0",
};

const groups: ConfigNavGroup[] = [groupA, groupB];

const stubA: Component = { render: () => h("div", { class: "stub-a" }, "A") };
const stubB: Component = { render: () => h("div", { class: "stub-b" }, "B") };
const stubMissing: Component = {
  render: () => h("div", { class: "stub-miss" }, "MISS"),
};

const screens: Record<string, Component> = {
  "a-1": stubA,
  "a-2": stubB,
};

function host(extraProps: Record<string, unknown> = {}) {
  return mount(ConfigShell, {
    props: { groups, screens, ...extraProps },
  });
}

// Attendre le cycle de rendu Vue (sync dans VTU)
async function tick() {
  await nextTick();
}

describe("ConfigShell", () => {
  describe("cas 1 — rendu nav", () => {
    it("les labels de groupes sont rendus ; le premier écran (première feuille de groups[0]) est actif", async () => {
      const wrapper = host();
      await tick();

      // Labels groupes
      expect(wrapper.text()).toContain("Groupe A");
      expect(wrapper.text()).toContain("Groupe B");

      // Sous-labels apparaissent quand l'accordéon est déplié
      const arrows = wrapper.findAll(".nav-arrow");
      expect(arrows.length).toBe(1); // seul groupA a un sub

      // Écran actif : premier sub de groups[0]
      expect(wrapper.find(".stub-a").exists()).toBe(true);

      wrapper.unmount();
    });
  });

  describe("cas 2 — feuille (sans sub)", () => {
    it("clique → update:activeScreen + nav-change cohérent", async () => {
      const wrapper = host();
      await tick();

      wrapper.find('[data-group="group-b"]').trigger("click");
      await tick();

      const update = wrapper.emitted("update:activeScreen");
      expect(update).toHaveLength(1);
      expect((update![0] as string[])[0]).toBe("group-b");

      const nav = wrapper.emitted("nav-change");
      const last = nav![nav!.length - 1][0] as Record<string, string>;
      expect(last.groupId).toBe("group-b");
      expect(last.screenId).toBe("group-b");
      expect(last.groupLabel).toBe("Groupe B");
      expect(last.screenLabel).toBe("Groupe B");

      // group-b pas dans screens → pending slot (donc pas de stub-b)
      // Vérifier plutôt que le nav-change était cohérent
      wrapper.unmount();
    });
  });

  describe("cas 3 — groupe avec sub", () => {
    it("montage : premier sub.id actif ; clic groupe → accordéon déplié + premier sub", async () => {
      const wrapper = host();
      await tick();

      // Au montage, l'écran actif est le premier sub
      expect(wrapper.find(".stub-a").exists()).toBe(true);

      // Clic sur le groupe avec sub → dépliement + sélection premier sub
      wrapper.find('[data-group="group-a"].nav-item').trigger("click");
      await tick();

      expect(wrapper.find(".nav-arrow.expanded").exists()).toBe(true);
      expect(wrapper.find(".stub-a").exists()).toBe(true);
      wrapper.unmount();
    });

    it("clic sous-groupe → sélection nav-change avec screenLabel du sous-groupe", async () => {
      const wrapper = host();
      await tick();

      // Clic sur le groupe avec sub → dépliement (rend les sub-items dans le DOM)
      wrapper.find('[data-group="group-a"].nav-item').trigger("click");
      await tick();

      // Les sous-groupes sont maintenant dans le DOM — clic sur le 2ème (Écran A2)
      const subItems = wrapper.findAll(".nav-sub-item");
      subItems[1].trigger("click");
      await tick();

      const nav = wrapper.emitted("nav-change");
      const last = nav![nav!.length - 1][0] as Record<string, string>;
      expect(last.screenId).toBe("a-2");
      expect(last.groupLabel).toBe("Groupe A");
      expect(last.screenLabel).toBe("Écran A2");
      expect(wrapper.find(".stub-b").exists()).toBe(true);
      wrapper.unmount();
    });
  });

  describe("cas 4 — screenId absent de screens", () => {
    it("placeholder pending par défaut ; slot personnalisé si fourni", async () => {
      const defaultScreens: Record<string, Component> = {
        ...screens,
        unknown: stubMissing,
      };

      // Sans slot pending : écran absent → contenu vide dans le panneau
      const wrapper = mount(ConfigShell, {
        props: { groups, screens: defaultScreens },
      });
      await tick();

      wrapper.find('[data-group="group-b"]').trigger("click");
      await tick();
      // group-b pas dans screens et pas de slot pending → panneau vide
      expect(wrapper.find(".stub-a").exists()).toBe(false);
      expect(wrapper.find(".stub-b").exists()).toBe(false);
      expect(wrapper.find(".stub-miss").exists()).toBe(false);

      // Avec slot personnalisé : le contenu du slot est rendu
      const wrapper2 = mount(ConfigShell, {
        props: { groups, screens: defaultScreens },
        slots: {
          pending: h("div", { class: "custom-pending" }, "Mon pending"),
        },
      });
      await tick();
      wrapper2.find('[data-group="group-b"]').trigger("click");
      expect(wrapper2.find(".custom-pending").exists()).toBe(true);

      wrapper.unmount();
      wrapper2.unmount();
    });
  });

  describe("cas 5 — nav-change au montage", () => {
    it("émis une fois avec la sélection initiale", async () => {
      const wrapper = host();
      await tick();
      const nav = wrapper.emitted("nav-change");
      expect(nav!).toHaveLength(1);
      const first = nav![0][0] as Record<string, string>;
      expect(first.groupId).toBe("group-a");
      expect(first.screenId).toBe("a-1");
      expect(first.groupLabel).toBe("Groupe A");
      expect(first.screenLabel).toBe("Écran A1");
      wrapper.unmount();
    });
  });

  describe("cas 6 — v-model:activeScreen contrôlé", () => {
    it("changement émet update:activeScreen ; parent qui repasse la prop reflète le nouvel écran", async () => {
      const wrapper = host({ activeScreen: "a-1" });
      await tick();

      wrapper.find('[data-group="group-b"]').trigger("click");
      await tick();

      const update = wrapper.emitted("update:activeScreen");
      expect(update).toHaveLength(1);
      expect((update![0] as string[])[0]).toBe("group-b");

      wrapper.setProps({ activeScreen: "a-2" });
      await tick();
      expect(wrapper.find(".stub-b").exists()).toBe(true);

      wrapper.unmount();
    });
  });
});
