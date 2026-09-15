const $k0=[1200n,'lay-col'];
const $k1=[6200n,'bg-t_app_bg'];
const $k2=[6400n,'fg-t_app_fg'];
const $k3=[$k0,$k1,$k2];
const $k4=[$k3];
const $k5=[5,$k4];
const $k6=[$k5];
const $k7=[0,false];
const $k8=[0,220n,38n,38n];
const $k9=[0,24n,24n,27n];
const $k10=[0,240n,240n,245n];
const $k11=[0,255n,255n,255n];
const $k12=[1200n,'lay-row'];
const $k13=[4200n,'px-r0_5'];
const $k14=[7400n,'r-6'];
const $k15=[6200n,'bg-t_cardlib_surface'];
const $k16=[6400n,'fg-t_cardlib_onSurface'];
const $k17=[6205n,'hover_bg-t_cardlib_danger'];
const $k18=[$k12,$k13,$k14,$k15,$k16,$k17];
const $k19=[$k18];
const $k20=[5,$k19];
const $k21=[$k20];
$ui_sheet='*,*::before,*::after{box-sizing:border-box}\n*,*::before,*::after{border-width:0}\n*{overflow-wrap:break-word}\n:where(body){margin:0}\n:where(div,nav,main,header,footer,aside,article,search,ul,li,hr,form,a,button){display:flex;flex-direction:column}\n:where(h1,h2,h3,h4,h5,h6){font-size:inherit;font-weight:inherit;margin:0}\n:where(button){appearance:none;background:none;border:0;padding:0;font:inherit;color:inherit}\n.lay-col{display:flex;flex-direction:column}\n.lay-row{display:flex;flex-direction:row}\n.px-r0_5{padding-inline:0.5rem}\n.bg-t_app_bg{background-color:var(--app-bg)}\n.bg-t_cardlib_surface{background-color:var(--cardlib-surface)}\n.hover_bg-t_cardlib_danger:hover{background-color:var(--cardlib-danger)}\n.fg-t_app_fg{color:var(--app-fg)}\n.fg-t_cardlib_onSurface{color:var(--cardlib-onSurface)}\n.r-6{border-radius:6px}\n';
$ui_theme_hook=$ui_theme_install;
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[],[],[]];
  const dark_1=[$host_HostUi_signal(ctx_0[2],false)];
  const self_6=$host_HostStdout_println(ctx_0[1],'mounted');
  let $t1;
  if(self_6[0]===0){
    $t1=0;
  }else if(self_6[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const label_10='clicks';
  const count_11=[$host_HostUi_signal(ctx_0[2],0n)];
  const config_23=[[0,label_10],[],void 0,c_12=>$host_HostUi_write(c_12[2],count_11[0],(n_13=>n_13+1n)($host_HostUi_read(c_12[2],count_11[0]))),void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0,void 0];
  const events_24=ui_node$listen$u3rqgv(config_23[8],config_23[9],config_23[10],config_23[11],config_23[12]);
  let $t3;
  const $t4=config_23[4];
  if($t4!==void 0&&$t4===1){
    const $t8=$share(config_23[1]);
    let $t5;
    const $t6=config_23[5];
    if($t6!==void 0){
      $t5=$t6;
    }else if($t6===void 0){
      $t5=$k7;
    }else{
      $abort('no arm matched');
    }
    $t3=[[14,config_23[0],$t8,$t5,events_24]];
  }else{
    const $t12=$share(config_23[1]);
    const children_27=$share(config_23[2]);
    let $t9;
    if(children_27!==void 0){
      $t9=$share(children_27);
    }else if(children_27===void 0){
      $t9=[];
    }else{
      $abort('no arm matched');
    }
    let $t13;
    const $t14=config_23[3];
    if($t14!==void 0){
      $t13=(c_31,_event_32)=>$t14(c_31);
    }else if($t14===void 0){
      $t13=(_c_33,_event_34)=>0;
    }else{
      $abort('no arm matched');
    }
    let $t15;
    const $t16=config_23[5];
    if($t16!==void 0){
      $t15=$t16;
    }else if($t16===void 0){
      $t15=$k7;
    }else{
      $abort('no arm matched');
    }
    let $t17;
    const $t18=config_23[6];
    if($t18!==void 0){
      $t17=$t18;
    }else if($t18===void 0){
      $t17=$k7;
    }else{
      $abort('no arm matched');
    }
    let $t19;
    const $t20=config_23[7];
    if($t20!==void 0){
      $t19=$t20;
    }else if($t20===void 0){
      $t19=$k7;
    }else{
      $abort('no arm matched');
    }
    $t3=[[5,config_23[0],$t12,$t9,$t13,$t15,$t17,$t19,events_24]];
  }
  const $t22=ui_node$stack$u3rqgv([$k6,[$t3,__cmd_x_main_buri$badge$u3rqgv([0,label_10],[1,count_11])],void 0,void 0,void 0,void 0,void 0,void 0]);
  const $t21=ui_theme$themed([[__cmd_x_main_buri$Card_color(0),__cmd_x_main_buri$cardTheme(0)],[__cmd_x_main_buri$Card_color(1),__cmd_x_main_buri$cardTheme(1)],[__cmd_x_main_buri$Card_color(2),__cmd_x_main_buri$cardTheme(2)]]);
  const whenTrue_16=__cmd_x_main_buri$appThemed(__cmd_x_main_buri$night);
  const whenFalse_17=__cmd_x_main_buri$appThemed(__cmd_x_main_buri$day);
  return $ui_node_mount(ctx_0,$t22,[$t21,[[1,[1,dark_1],[whenTrue_16],[whenFalse_17]]]]);
}
function __cmd_x_main_buri$cardTheme(t_0){
  switch(t_0){
    case 0:
      {
        return __cmd_x_main_buri$App_color(0);
      }
    case 1:
      {
        return __cmd_x_main_buri$App_color(1);
      }
    case 2:
      {
        return $k8;
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function __cmd_x_main_buri$night(t_0){
  if(t_0===0){
    return $k9;
  }else if(t_0===1){
    return $k10;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$appThemed(f_0){
  const bindings_1=[[__cmd_x_main_buri$App_color(0),f_0(0)],[__cmd_x_main_buri$App_color(1),f_0(1)]];
  return [[0,bindings_1]];
}
function __cmd_x_main_buri$day(t_0){
  if(t_0===0){
    return $k11;
  }else if(t_0===1){
    return $k9;
  }else{
    $abort('no arm matched');
  }
}
function __cmd_x_main_buri$App_color(self_0){
  if(self_0===0){
    return [2,['app','bg']];
  }else if(self_0===1){
    return [2,['app','fg']];
  }else{
    $abort('no arm matched');
  }
}
function ui_theme$themed(bindings_0){
  return [[0,bindings_0]];
}
function __cmd_x_main_buri$Card_color(self_0){
  switch(self_0){
    case 0:
      {
        return [2,['cardlib','surface']];
      }
    case 1:
      {
        return [2,['cardlib','onSurface']];
      }
    case 2:
      {
        return [2,['cardlib','danger']];
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
function __cmd_x_main_buri$badge$u3rqgv(title_0,count_1){
  return ui_node$stack$u3rqgv([$k21,[ui_node$text$u3rqgv([title_0,void 0,void 0]),ui_node$text$u3rqgv([[2,c_2=>{
    let $t1;
    if(count_1[0]===0){
      $t1=count_1[1];
    }else if(count_1[0]===1){
      $t1=$ui_effect_Scope_read(c_2,count_1[1][0]);
    }else if(count_1[0]===2){
      $t1=count_1[1](c_2);
    }else{
      $abort('no arm matched');
    }
    return String($t1);
  }],void 0,void 0])],void 0,void 0,void 0,void 0,void 0,void 0]);
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
function ui_node$listen$u3rqgv(onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4){
  return [onHover_0,onFocus_1,onScroll_2,onKey_3,onPressOutside_4];
}
function ui_node$text$u3rqgv(config_0){
  const $t1=config_0[1];
  if($t1!==void 0){
    const styles_2=$share(config_0[2]);
    let $t2;
    if(styles_2!==void 0){
      $t2=$share(styles_2);
    }else if(styles_2===void 0){
      $t2=[];
    }else{
      $abort('no arm matched');
    }
    return [[2,$t1,$t2,config_0[0]]];
  }else if($t1===void 0){
    return [[1,config_0[0]]];
  }else{
    $abort('no arm matched');
  }
}
