const $k0=[1,'x'];
const $k1=[1,'y'];
const $k2=[0];
const $k3=[1,2];
const $k4=[0,0];
const $D0=[];
const $D1=[];
const $D2=[];
const $D3=[];
$D0.push(2,'Pair',true,['a','b'],[$D1,$D2]);
$D1.push(0,'i');
$D2.push(0,'s');
$D3.push(3,'Tag',[['Low',false,[],[]],['High',false,['0'],[$D1]]],false);
function $eqD0(a,b){
  if(a===b){
    return true;
  }
  return a[0]===b[0]&&a[1]===b[1];
}
function $eqD3(a,b){
  if(a===b){
    return true;
  }
  if(a[0]!==b[0]){
    return false;
  }
  switch(a[0]){
    case 0:
      return true;
    case 1:
      return a[1]===b[1];
    default:
      return false;
  }
}
function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],$str($eqD0($k0,$k1))+' '+$str($eqD0($k0,$k0)));
  $host_HostStdout_println(ctx_0[1],$show($k0,$D0)+' '+$show($k1,$D0));
  $host_HostStdout_println(ctx_0[1],$str($eqD3($k2,$k3))+' '+$show($k3,$D3));
  return $k4;
}
