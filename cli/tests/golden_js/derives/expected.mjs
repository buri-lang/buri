const $D0=[];
const $D1=[];
const $D2=[];
const $D3=[];
$D0.push(2,'Pair',true,['a','b'],[$D1,$D2]);
$D1.push(0,'i');
$D2.push(0,'s');
$D3.push(3,'Tag',[['Low',false,[],[]],['High',false,['0'],[$D1]]],false);
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const p_1=[1,'x'];
  const q_2=[1,'y'];
  $host_HostStdout_println(ctx_0[1],[$eq(p_1,q_2),' ',$eq(p_1,p_1)]);
  $host_HostStdout_println(ctx_0[1],[$show(p_1,$D0),' ',$show(q_2,$D0)]);
  $host_HostStdout_println(ctx_0[1],[$eq([0],[1,2]),' ',$show([1,2],$D3)]);
  return [0,0];
}
