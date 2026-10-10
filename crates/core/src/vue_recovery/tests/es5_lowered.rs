//! ES5-lowered Vue compiler output: render closures, slots, and handlers are
//! `function` expressions. Inputs are producer @vue/compiler-sfc@3.5.35
//! (inline template, production) lowered by @babel/preset-env@7.29.7 with
//! targets ie 11 and modules false.

use super::*;

/// The render closure `setup` returns, with a hoisted handler binding.
#[test]
fn recovers_es5_lowered_setup_render_closure_and_handler() {
    let input = r#"
import { toDisplayString as _toDisplayString, normalizeClass as _normalizeClass, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
var __sfc__ = {
  __name: 'Component2',
  props: {
    active: Boolean,
    count: Number
  },
  emits: ["increment"],
  setup: function setup(__props, _ref) {
    var __emit = _ref.emit;
    var props = __props;
    var emit = __emit;
    function increment() {
      emit("increment");
    }
    return function (_ctx, _cache) {
      return _openBlock(), _createElementBlock("button", {
        class: _normalizeClass(["counter", {
          active: props.active
        }]),
        onClick: increment
      }, _toDisplayString(props.count), 3);
    };
  }
};
export default __sfc__;
"#;

    assert_eq!(
        decompile_sfc(input, DecompileOptions::default())
            .unwrap()
            .code,
        r#"<script setup>
const props = defineProps({
    active: Boolean,
    count: Number
});
const { active, count } = props;

const emit = defineEmits([
    "increment"
]);

function increment() {
    emit("increment");
}
</script>

<template>
  <button class="counter" :class="{ active }" @click="increment">{{ count }}</button>
</template>
"#
    );
}

/// A `renderList` item callback.
#[test]
fn recovers_es5_lowered_render_list_callback() {
    let input = r#"
import { renderList as _renderList, Fragment as _Fragment, openBlock as _openBlock, createElementBlock as _createElementBlock, toDisplayString as _toDisplayString } from "vue";
var __sfc__ = {
  __name: 'Component4',
  props: {
    items: Array
  },
  setup: function setup(__props) {
    return function (_ctx, _cache) {
      return _openBlock(), _createElementBlock("ul", null, [(_openBlock(true), _createElementBlock(_Fragment, null, _renderList(__props.items, function (item) {
        return _openBlock(), _createElementBlock("li", {
          key: item.id
        }, _toDisplayString(item.name), 1);
      }), 128))]);
    };
  }
};
export default __sfc__;
"#;

    assert_eq!(
        decompile_sfc(input, DecompileOptions::default())
            .unwrap()
            .code,
        r#"<script setup>
const props = defineProps({
    items: Array
});
const { items } = props;
</script>

<template>
  <ul>
    <li v-for="item in items" :key="item.id">{{ item.name }}</li>
  </ul>
</template>
"#
    );
}

/// A `renderSlot` fallback closure.
#[test]
fn recovers_es5_lowered_slot_fallback() {
    let input = r#"
import { createVNode as _createVNode, renderSlot as _renderSlot, createTextVNode as _createTextVNode, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
import PanelHeader from "./PanelHeader.vue";
var __sfc__ = {
  __name: 'Component5',
  props: {
    title: String
  },
  setup: function setup(__props) {
    return function (_ctx, _cache) {
      return _openBlock(), _createElementBlock("article", null, [_createVNode(PanelHeader, {
        title: __props.title
      }, null, 8, ["title"]), _renderSlot(_ctx.$slots, "body", {}, function () {
        return [_cache[0] || (_cache[0] = _createTextVNode("Empty", -1))];
      })]);
    };
  }
};
export default __sfc__;
"#;

    assert_eq!(
        decompile_sfc(input, DecompileOptions::default())
            .unwrap()
            .code,
        r#"<script setup>
import PanelHeader from "./PanelHeader.vue";

const props = defineProps({
    title: String
});
const { title } = props;
</script>

<template>
  <article>
    <PanelHeader :title="title" />
    <slot name="body">Empty</slot>
  </article>
</template>
"#
    );
}

/// `withCtx` slot closures with destructured slot props, and an inline handler.
#[test]
fn recovers_es5_lowered_scoped_slots_and_inline_handler() {
    let input = r#"
function _toConsumableArray(r) { return _arrayWithoutHoles(r) || _iterableToArray(r) || _unsupportedIterableToArray(r) || _nonIterableSpread(); }
function _nonIterableSpread() { throw new TypeError("Invalid attempt to spread non-iterable instance.\nIn order to be iterable, non-array objects must have a [Symbol.iterator]() method."); }
function _unsupportedIterableToArray(r, a) { if (r) { if ("string" == typeof r) return _arrayLikeToArray(r, a); var t = {}.toString.call(r).slice(8, -1); return "Object" === t && r.constructor && (t = r.constructor.name), "Map" === t || "Set" === t ? Array.from(r) : "Arguments" === t || /^(?:Ui|I)nt(?:8|16|32)(?:Clamped)?Array$/.test(t) ? _arrayLikeToArray(r, a) : void 0; } }
function _iterableToArray(r) { if ("undefined" != typeof Symbol && null != r[Symbol.iterator] || null != r["@@iterator"]) return Array.from(r); }
function _arrayWithoutHoles(r) { if (Array.isArray(r)) return _arrayLikeToArray(r); }
function _arrayLikeToArray(r, a) { (null == a || a > r.length) && (a = r.length); for (var e = 0, n = Array(a); e < a; e++) n[e] = r[e]; return n; }
import { toDisplayString as _toDisplayString, createElementVNode as _createElementVNode, createTextVNode as _createTextVNode, withCtx as _withCtx, openBlock as _openBlock, createBlock as _createBlock } from "vue";
var _hoisted_1 = ["data-index", "onClick"];
import DataList from "./DataList.vue";
var __sfc__ = {
  __name: 'Component6',
  props: {
    items: Array
  },
  setup: function setup(__props) {
    function select(item) {
      return item.id;
    }
    return function (_ctx, _cache) {
      return _openBlock(), _createBlock(DataList, {
        items: __props.items
      }, {
        default: _withCtx(function (_ref) {
          var item = _ref.item,
            index = _ref.index;
          return [_createElementVNode("button", {
            "data-index": index,
            onClick: function onClick($event) {
              return select(item);
            }
          }, _toDisplayString(item.name), 9, _hoisted_1)];
        }),
        empty: _withCtx(function () {
          return _toConsumableArray(_cache[0] || (_cache[0] = [_createTextVNode("No items", -1)]));
        }),
        _: 1
      }, 8, ["items"]);
    };
  }
};
export default __sfc__;
"#;

    assert_eq!(
        decompile_sfc(input, DecompileOptions::default())
            .unwrap()
            .code,
        r#"<script setup>
import DataList from "./DataList.vue";

const props = defineProps({
    items: Array
});
const { items } = props;

function select(item) {
    return item.id;
}
</script>

<template>
  <DataList :items="items">
    <template v-slot:default="{ item, index }">
      <button :data-index="index" @click="select(item)">{{ item.name }}</button>
    </template>
    <template v-slot:empty>No items</template>
  </DataList>
</template>
"#
    );
}

/// A cached `onUpdate:modelValue` handler.
#[test]
fn recovers_es5_lowered_model_update_handler() {
    let input = r#"
import { useModel as _useModel } from 'vue';
import { vModelText as _vModelText, vShow as _vShow, withDirectives as _withDirectives, openBlock as _openBlock, createElementBlock as _createElementBlock } from "vue";
var visible = true;
var __sfc__ = {
  __name: 'Component7',
  props: {
    "modelValue": {},
    "modelModifiers": {}
  },
  emits: ["update:modelValue"],
  setup: function setup(__props) {
    var value = _useModel(__props, "modelValue");
    return function (_ctx, _cache) {
      return _withDirectives((_openBlock(), _createElementBlock("input", {
        "onUpdate:modelValue": _cache[0] || (_cache[0] = function ($event) {
          return value.value = $event;
        })
      }, null, 512)), [[_vModelText, value.value], [_vShow, visible]]);
    };
  }
};
export default __sfc__;
"#;

    assert_eq!(
        decompile_sfc(input, DecompileOptions::default())
            .unwrap()
            .code,
        r#"<script setup>
import { useModel } from "vue";

const props = defineProps({
    modelValue: {},
    modelModifiers: {}
});
const { modelModifiers, modelValue } = props;

const visible = true;
const value = useModel(props, "modelValue");
</script>

<template>
  <input v-model="value" v-show="visible" />
</template>
"#
    );
}
