<script setup lang="ts">
import { type AppConfig } from "../api";
import ModelsView from "../views/ModelsView.vue";

const props = defineProps<{
  cfg: AppConfig;
  saveModelConfig: () => Promise<boolean | void>;
}>();

</script>

<template>
  <div class="settings-model-tab">
    <div class="embedded-view model-embed"><ModelsView :cfg="props.cfg" :save-model-config="props.saveModelConfig" /></div>
  </div>
</template>

