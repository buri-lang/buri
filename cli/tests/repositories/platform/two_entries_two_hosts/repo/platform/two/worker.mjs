import { worker } from "buri:program";

let n = 0n;
export default { fetch(request) { return worker(request); } };
export const HostCount = {
  next: (self) => ++n,
};
