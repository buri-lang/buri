const $k0=[1200n,'lay-col'];
const $k1=[4000n,'p-r1'];
const $k2=[3400n,'gap-8'];
const $k3=[4002n,'md_p-r2'];
const $k4=[3402n,'md_gap-16'];
const $k5=[5403n,'lg_maxw-r64'];
const $k6=[7406n,'sm_hover_r-4'];
const $k7=[$k0,$k1,$k2,$k3,$k4,$k5,$k6];
const $k8=[$k7];
const $k9=[5,$k8];
const $k10=[$k9];
const $k11=[1200n,'lay-row'];
const $k12=[4200n,'px-r0_5'];
const $k13=[7400n,'r-6'];
const $k14=[6200n,'bg-f0f0f5'];
const $k15=[6400n,'fg-18181b'];
const $k16=[6205n,'hover_bg-18181b'];
const $k17=[6405n,'hover_fg-f0f0f5'];
const $k18=[$k11,$k12,$k13,$k14,$k15,$k16,$k17];
const $k19=[$k18];
const $k20=[5,$k19];
const $k21=[$k20];
const $k22=[2400n,'grow-1'];
const $k23=[$k11,$k12,$k22];
const $k24=[$k23];
const $k25=[5,$k24];
const $k26=[$k25];
const $k27=[$k26,[],void 0,void 0,void 0,void 0,void 0,void 0];
const $k29=[2600n,'shrink-0'];
const $k30=[$k29];
const $k31=[$k30];
const $k32=[5,$k31];
const $k33=[void 0,void 0,void 0,void 0,void 0];
const $k34=[$k22];
const $k35=[$k34];
const $k36=[5,$k35];
const $k37=[$k36];
const $k38=[3,$k37,[],$k33];
const $k39=[$k38];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.grow-1{flex-grow:1}\n.shrink-0{flex-shrink:0}\n.gap-8{gap:8px}\n.p-r1{padding:1rem}\n.px-r0_5{padding-inline:0.5rem}\n.w-var{width:var(--buri-w)}\n.h-var{height:var(--buri-h)}\n.bg-f0f0f5{background-color:rgb(240,240,245)}\n.hover_bg-18181b:hover{background-color:rgb(24,24,27)}\n.fg-18181b{color:rgb(24,24,27)}\n.hover_fg-f0f0f5:hover{color:rgb(240,240,245)}\n.r-6{border-radius:6px}\n@media (min-width:40rem){\n.sm_hover_r-4:hover{border-radius:4px}\n}\n@media (min-width:48rem){\n.md_gap-16{gap:16px}\n.md_p-r2{padding:2rem}\n}\n@media (min-width:64rem){\n.lg_maxw-r64{max-width:64rem}\n}\n';
$tree_declare_hook=$tree_declare;
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const self_3=$host_HostStdout_println(ctx_0[1],'styled');
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const config_9=[[0,'one'],void 0,void 0];
  let $t3;
  const $t4=config_9[1];
  if($t4!==void 0){
    const styles_11=$share(config_9[2]);
    let $t5;
    if(styles_11!==void 0){
      $t5=$share(styles_11);
    }else if(styles_11===void 0){
      $t5=[];
    }else{
      $abort('no arm matched');
    }
    $t3=[[2,$t4,$t5,config_9[0]]];
  }else if($t4===void 0){
    $t3=[[1,config_9[0]]];
  }else{
    $abort('no arm matched');
  }
  const $t9=ui_node$stack$u3rqgv([$k21,[$t3],void 0,void 0,void 0,void 0,void 0,void 0]);
  const $t10=ui_node$stack$u3rqgv($k27);
  let $t7;
  const $t8=void 0;
  if($t8!==void 0){
    $t7=[[3,[[24,$t8],[25,$t8],$k32],[],$k33]];
  }else if($t8===void 0){
    $t7=$k39;
  }else{
    $abort('no arm matched');
  }
  return $ui_node_mount(ctx_0,ui_node$stack$u3rqgv([$k10,[$t9,$t10,$t7],void 0,void 0,void 0,void 0,void 0,void 0]),[]);
}
function ui_node$stack$u3rqgv(config_0){
  const events_1=[config_0[3],config_0[4],config_0[5],config_0[6],config_0[7]];
  const $t1=config_0[2];
  if($t1!==void 0){
    return [[4,$t1,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else if($t1===void 0){
    return [[3,$share(config_0[0]),$share(config_0[1]),events_1]];
  }else{
    $abort('no arm matched');
  }
}
