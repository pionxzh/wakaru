import ms from "ms";
import { CartStore } from "./cart.js";

const cart = new CartStore();

document.querySelector("#add").addEventListener("click", async () => {
  await cart.add("sku-42", 1);
  document.querySelector("#total").textContent = cart.total().toFixed(2);
});

document.querySelector("#checkout").addEventListener("click", async () => {
  const { checkout } = await import("./checkout.js");
  await checkout(cart);
});

setTimeout(() => cart.clear(), ms("2h"));
